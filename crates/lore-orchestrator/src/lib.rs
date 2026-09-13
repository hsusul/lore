//! # lore-orchestrator
//!
//! Runs coding agents in parallel, each in its own Lore-owned git worktree
//! (ADR-0007, `docs/product/ORCHESTRATOR_PLAN.md`).
//!
//! Boundaries:
//! - Worktrees and logs live only under `<root>/worktrees` and `<root>/logs`;
//!   discard refuses any path outside them and only deletes `lore/` branches.
//! - The user's primary checkout gains a branch plus worktree metadata; its
//!   files change only through an explicit `merge_task`, which refuses on a
//!   dirty checkout and restores it on any failure.
//! - Agents are launched without permission-bypass flags (see [`agent`]), each
//!   as its own process group so stopping a task stops everything it started.
//! - No archive access and no network: this crate only spawns local processes.
//! - One orchestrator per root: `open` takes an exclusive lock on `<root>/lock`.
//!
//! Locking: every method takes `&self`. The internal mutex guards only task
//! records and child handles. Git commands, process waits, and log parsing run
//! without it; a per-task busy marker keeps two operations off the same task.
//! Git-derived status comes from [`describe`] on a [`TaskSnapshot`].

pub mod agent;
mod git;
mod process;
pub mod workspace;

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lore_ipc::{
    ActivityDto, ActivityKind, ContinueTaskRequest, CreateTaskRequest, MergeResultDto, TaskAgent,
    TaskDiffDto, TaskDto, TaskOverlapDto, TaskPermission, TaskState,
};
use serde::{Deserialize, Serialize};

pub use agent::AgentPrograms;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Git(String),
    #[error("task not found")]
    NotFound,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("task store could not be written: {0}")]
    Store(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

const STORE_FILE: &str = "tasks.json";
const LOCK_FILE: &str = "lock";
const BRANCH_PREFIX: &str = "lore/";
const MAX_TITLE: usize = 200;
const MAX_PROMPT: usize = 100_000;
const MAX_CHANGED_FILES: usize = 1_000;
/// How long helper processes may outlive a finished agent before SIGKILL.
const LINGER_GRACE: Duration = Duration::from_secs(3);

/// A task as persisted on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskRecord {
    id: String,
    title: String,
    prompt: String,
    agent: TaskAgent,
    state: TaskState,
    repo_path: PathBuf,
    worktree_path: PathBuf,
    branch: String,
    base_commit: String,
    log_path: PathBuf,
    created_at_ms: i64,
    exit_code: Option<i64>,
    /// Agent pid, which is also its process-group id.
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    permission: TaskPermission,
    /// Runs in this worktree: the first launch plus each continuation.
    #[serde(default = "one")]
    runs: u32,
    /// Branch of the primary checkout this task was merged into.
    #[serde(default)]
    merged_into: Option<String>,
}

fn one() -> u32 {
    1
}

/// A point-in-time copy of one task, safe to [`describe`] without holding the
/// orchestrator.
#[derive(Debug, Clone)]
pub struct TaskSnapshot(TaskRecord);

impl TaskSnapshot {
    pub fn id(&self) -> &str {
        &self.0.id
    }
    pub fn worktree_path(&self) -> &Path {
        &self.0.worktree_path
    }
}

struct Inner {
    programs: AgentPrograms,
    tasks: Vec<TaskRecord>,
    children: HashMap<String, Child>,
    /// Tasks with an operation in progress outside the lock.
    busy: HashSet<String>,
    /// Process groups whose leader exited but which may still have helpers:
    /// group id → deadline for SIGKILL.
    lingering: HashMap<u32, Instant>,
    /// Workspace roots the user opened (canonical).
    opened_roots: HashSet<PathBuf>,
    load_warning: Option<String>,
}

/// Owns task records, their worktrees, and the agent processes it started.
pub struct Orchestrator {
    root: PathBuf,
    inner: Mutex<Inner>,
    _instance: InstanceLock,
}

/// One orchestrator per root, across and within processes.
///
/// Within a process, a registry of open roots. Across processes, `<root>/lock`
/// records the owner's pid; a live owner running the same executable blocks
/// opening. (An `flock` was tried first, but descriptors inherited by children
/// forked elsewhere in the process kept it held after the owner closed it.)
struct InstanceLock {
    root: PathBuf,
}

fn open_roots() -> &'static Mutex<HashSet<PathBuf>> {
    static ROOTS: std::sync::OnceLock<Mutex<HashSet<PathBuf>>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| Mutex::new(HashSet::new()))
}

impl InstanceLock {
    fn acquire(root: &Path) -> Result<Self> {
        let already = || {
            Error::Invalid("Lore is already running (another window holds the task list)".into())
        };
        let root = fs::canonicalize(root)?;
        let lock_path = root.join(LOCK_FILE);
        if let Some(pid) = fs::read_to_string(&lock_path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            let me = std::process::id();
            if pid != me && process::group_alive_pid(pid) {
                if let Ok(exe) = std::env::current_exe() {
                    if process::runs_program(pid, &exe) {
                        return Err(already());
                    }
                }
            }
        }
        {
            let mut roots = open_roots().lock().unwrap_or_else(|p| p.into_inner());
            if !roots.insert(root.clone()) {
                return Err(already());
            }
        }
        if let Err(e) = fs::write(&lock_path, std::process::id().to_string()) {
            open_roots()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&root);
            return Err(e.into());
        }
        Ok(Self { root })
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.root.join(LOCK_FILE));
        open_roots()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.root);
    }
}

/// Clears a task's busy marker when an operation ends, however it ends.
struct Busy<'a> {
    orchestrator: &'a Orchestrator,
    id: String,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        let mut inner = self.orchestrator.lock();
        inner.busy.remove(&self.id);
    }
}

impl Orchestrator {
    /// Open (or create) the orchestrator rooted at `root`.
    ///
    /// Fails if another process already has this root open. Never fails because
    /// of a damaged store: an unreadable `tasks.json` is moved aside and reported
    /// via [`Orchestrator::load_warning`]. Tasks that were running when the
    /// previous process exited become `Interrupted`, and any of their agents
    /// still alive are stopped.
    pub fn open(root: impl Into<PathBuf>, programs: AgentPrograms) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("worktrees"))?;
        fs::create_dir_all(root.join("logs"))?;
        let instance = InstanceLock::acquire(&root)?;

        let store = root.join(STORE_FILE);
        let mut load_warning = None;
        let mut tasks: Vec<TaskRecord> = match fs::read(&store) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(tasks) => tasks,
                Err(e) => {
                    let aside = root.join(format!("{STORE_FILE}.corrupt-{}", now_ms()));
                    let _ = fs::rename(&store, &aside);
                    load_warning = Some(format!(
                        "The task list could not be read ({e}) and was moved to {}. \
                         Worktrees on disk were left untouched.",
                        aside.display()
                    ));
                    Vec::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };

        let mut changed = false;
        for task in &mut tasks {
            if task.state == TaskState::Running {
                if let Some(pid) = task.pid {
                    if process::group_alive(pid)
                        && process::runs_program(pid, program_for(&programs, task.agent))
                    {
                        process::stop_group(pid, None);
                    }
                }
                task.state = TaskState::Interrupted;
                changed = true;
            }
        }
        let orchestrator = Self {
            root,
            inner: Mutex::new(Inner {
                programs,
                tasks,
                children: HashMap::new(),
                busy: HashSet::new(),
                lingering: HashMap::new(),
                opened_roots: HashSet::new(),
                load_warning,
            }),
            _instance: instance,
        };
        if changed {
            let inner = orchestrator.lock();
            orchestrator.save(&inner)?;
        }
        Ok(orchestrator)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves records consistent enough to
        // keep serving; recover rather than failing every later call.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Why previously saved tasks could not be loaded, if that happened.
    pub fn load_warning(&self) -> Option<String> {
        self.lock().load_warning.clone()
    }

    /// Replace the programs used for tasks launched from now on.
    pub fn set_programs(&self, programs: AgentPrograms) {
        self.lock().programs = programs;
    }

    /// Record a workspace root the user opened, allowing it to be browsed.
    /// Returns the repository top-level.
    pub fn open_workspace(&self, path: &str) -> Result<PathBuf> {
        let top = workspace::repository_root(path)?;
        self.lock().opened_roots.insert(top.clone());
        Ok(top)
    }

    /// Validate a root the webview wants to browse: an opened workspace, the
    /// repository of a task, or a task worktree. Returns the canonical root.
    pub fn browsable_root(&self, root: &str) -> Result<PathBuf> {
        let canonical = workspace::workspace_root(root)?;
        let inner = self.lock();
        let allowed = inner.opened_roots.contains(&canonical)
            || inner.tasks.iter().any(|t| {
                [&t.repo_path, &t.worktree_path]
                    .iter()
                    .any(|p| fs::canonicalize(p).is_ok_and(|p| p == canonical))
            });
        if allowed {
            Ok(canonical)
        } else {
            Err(Error::Invalid(
                "open this folder in Lore before browsing it".into(),
            ))
        }
    }

    fn mark_busy(&self, inner: &mut Inner, id: &str) -> Result<Busy<'_>> {
        if !inner.busy.insert(id.to_string()) {
            return Err(Error::Invalid(
                "another action on this task is still in progress".into(),
            ));
        }
        Ok(Busy {
            orchestrator: self,
            id: id.to_string(),
        })
    }

    /// Create a worktree for the request and launch its agent there.
    pub fn create_task(&self, req: &CreateTaskRequest) -> Result<TaskSnapshot> {
        let title = req.title.trim();
        let prompt = req.prompt.trim();
        if title.is_empty()
            || title.chars().count() > MAX_TITLE
            || title.chars().any(char::is_control)
        {
            return Err(Error::Invalid(
                "title must be 1-200 printable characters".into(),
            ));
        }
        if prompt.is_empty() || prompt.len() > MAX_PROMPT {
            return Err(Error::Invalid(
                "prompt must be non-empty and under 100 KB".into(),
            ));
        }
        let requested = PathBuf::from(&req.repo_path);
        if !requested.is_absolute() || !requested.is_dir() {
            return Err(Error::Invalid(
                "repository path must be an existing absolute directory".into(),
            ));
        }

        // Git work, without the lock.
        let repo = git::toplevel(&requested)?;
        let base_commit = git::head_commit(&repo)?;
        let created_at_ms = now_ms();
        let id = new_id(created_at_ms);
        let worktree_path = self.root.join("worktrees").join(&id);
        let log_path = self.root.join("logs").join(format!("{id}.log"));
        let preferred = format!("{BRANCH_PREFIX}{}", slug(title));
        let fallback = format!("{preferred}-{id}");
        let branch = if !git::branch_exists(&repo, &preferred)
            && git::add_worktree(&repo, &worktree_path, &preferred, &base_commit).is_ok()
        {
            preferred
        } else {
            git::add_worktree(&repo, &worktree_path, &fallback, &base_commit)?;
            fallback
        };

        let mut record = TaskRecord {
            id: id.clone(),
            title: title.to_string(),
            prompt: prompt.to_string(),
            agent: req.agent,
            state: TaskState::Running,
            repo_path: repo,
            worktree_path,
            branch,
            base_commit,
            log_path,
            created_at_ms,
            exit_code: None,
            pid: None,
            permission: req.permission.unwrap_or_default(),
            runs: 1,
            merged_into: None,
        };

        let mut inner = self.lock();
        match spawn(&inner.programs, &record, &record.prompt, None) {
            Ok(child) => {
                record.pid = Some(child.id());
                inner.children.insert(id.clone(), child);
            }
            Err(e) => {
                record.state = TaskState::Failed;
                let _ = fs::write(
                    &record.log_path,
                    format!("Lore could not start the agent: {e}\n"),
                );
            }
        }
        if let Ok(top) = fs::canonicalize(&record.repo_path) {
            inner.opened_roots.insert(top);
        }
        inner.tasks.push(record);
        if let Err(e) = self.save(&inner) {
            // Without a record nothing could ever clean this up, so roll back.
            drop(inner);
            let _ = self.discard_task(&id);
            return Err(e);
        }
        Ok(TaskSnapshot(
            inner.tasks.last().cloned().ok_or(Error::NotFound)?,
        ))
    }

    /// Send more work to a task that is not running, in the same worktree.
    ///
    /// Keeping the agent resumes its own session when it supports that (Claude);
    /// otherwise, including every switch of agent (a handoff), the new run gets a
    /// brief built from the task's commits, changes, and recent activity.
    pub fn continue_task(&self, req: &ContinueTaskRequest) -> Result<TaskSnapshot> {
        let prompt = req.prompt.trim();
        if prompt.is_empty() || prompt.len() > MAX_PROMPT {
            return Err(Error::Invalid(
                "prompt must be non-empty and under 100 KB".into(),
            ));
        }
        let (task, _busy) = {
            let mut inner = self.lock();
            self.reap(&mut inner)?;
            let task = inner.task(&req.id)?.clone();
            ensure_idle(&inner, &task)?;
            if task.merged_into.is_some() {
                return Err(Error::Invalid("the task was already merged".into()));
            }
            let busy = self.mark_busy(&mut inner, &req.id)?;
            (task, busy)
        };
        if !task.worktree_path.is_dir() {
            return Err(Error::Invalid(
                "the task's worktree no longer exists".into(),
            ));
        }

        // Log parsing and git for the brief, without the lock.
        let new_agent = req.agent.unwrap_or(task.agent);
        let session = (new_agent == task.agent && new_agent == TaskAgent::ClaudeCode)
            .then(|| agent::claude_session_id(&task.log_path))
            .flatten();
        let full_prompt = match session {
            Some(_) => prompt.to_string(),
            None => handoff_brief(&task, prompt),
        };

        let mut inner = self.lock();
        let index = inner.index(&req.id)?;
        let previous = inner.tasks[index].clone();
        let mut record = previous.clone();
        record.agent = new_agent;
        record.runs += 1;
        record.exit_code = None;
        let child = match spawn(&inner.programs, &record, &full_prompt, session.as_deref()) {
            Ok(child) => {
                record.pid = Some(child.id());
                record.state = TaskState::Running;
                Some(child)
            }
            Err(e) => {
                record.state = TaskState::Failed;
                record.pid = None;
                let _ = fs::OpenOptions::new()
                    .append(true)
                    .open(&record.log_path)
                    .and_then(|mut f| writeln!(f, "Lore could not start the agent: {e}"));
                None
            }
        };
        inner.tasks[index] = record;
        if let Err(e) = self.save(&inner) {
            // Keep memory and disk in agreement: undo the run.
            if let Some(mut child) = child {
                let pgid = child.id();
                process::stop_group(pgid, Some(&mut child));
            }
            inner.tasks[index] = previous;
            return Err(e);
        }
        if let Some(child) = child {
            inner.children.insert(req.id.clone(), child);
        }
        Ok(TaskSnapshot(inner.tasks[index].clone()))
    }

    /// Commit every uncommitted change in the task's worktree.
    pub fn commit_task(&self, id: &str, message: &str) -> Result<TaskSnapshot> {
        let (task, _busy) = {
            let mut inner = self.lock();
            self.reap(&mut inner)?;
            let task = inner.task(id)?.clone();
            ensure_idle(&inner, &task)?;
            let busy = self.mark_busy(&mut inner, id)?;
            (task, busy)
        };
        let message = message.trim();
        let message = if message.is_empty() {
            task.title.clone()
        } else {
            message.to_string()
        };
        if message.len() > 10_000 {
            return Err(Error::Invalid("commit message is too long".into()));
        }
        if !git::commit_all(&task.worktree_path, &message)? {
            return Err(Error::Invalid("there is nothing to commit".into()));
        }
        Ok(TaskSnapshot(task))
    }

    /// Merge the task's branch into the branch checked out in its repository.
    ///
    /// Refuses while the agent runs, while the worktree has uncommitted changes,
    /// or while the primary checkout has uncommitted changes. A conflicting merge
    /// is aborted, leaving the checkout exactly as it was.
    pub fn merge_task(&self, id: &str) -> Result<MergeResultDto> {
        let (task, _busy) = {
            let mut inner = self.lock();
            self.reap(&mut inner)?;
            let task = inner.task(id)?.clone();
            ensure_idle(&inner, &task)?;
            if let Some(into) = &task.merged_into {
                return Err(Error::Invalid(format!(
                    "the task was already merged into {into}"
                )));
            }
            let busy = self.mark_busy(&mut inner, id)?;
            (task, busy)
        };
        if !task.branch.starts_with(BRANCH_PREFIX) {
            return Err(Error::Invalid(
                "refusing to merge a branch Lore did not create".into(),
            ));
        }
        if git::checked_out_branch(&task.worktree_path).as_deref() != Some(task.branch.as_str()) {
            return Err(Error::Invalid(format!(
                "the task's worktree is no longer on {}; check out that branch there first",
                task.branch
            )));
        }
        if !git::uncommitted(&task.worktree_path).is_empty() {
            return Err(Error::Invalid(
                "the task has uncommitted changes; commit them first".into(),
            ));
        }
        let into = git::current_branch(&task.repo_path)?;
        let dirty = git::uncommitted(&task.repo_path);
        if !dirty.is_empty() {
            return Err(Error::Invalid(format!(
                "your checkout on {into} has uncommitted changes ({} files); commit or stash them first",
                dirty.len()
            )));
        }
        if git::branch_commits_ahead(&task.repo_path, &task.base_commit, &task.branch) == 0 {
            return Err(Error::Invalid("the task has no commits to merge".into()));
        }
        if !git::has_identity(&task.repo_path) {
            return Err(Error::Invalid(
                "git has no user.name/user.email configured for this repository".into(),
            ));
        }
        let message = format!("Merge {} ({})", task.branch, task.title);
        match git::merge_branch(&task.repo_path, &task.branch, &message)? {
            Ok(()) => {
                let mut inner = self.lock();
                let mut note = String::new();
                if let Ok(index) = inner.index(id) {
                    inner.tasks[index].merged_into = Some(into.clone());
                    // The merge already happened in git; a failed save must not
                    // turn it into an error the user might retry.
                    if let Err(e) = self.save(&inner) {
                        note = format!(" (Lore could not save this: {e})");
                    }
                }
                Ok(MergeResultDto {
                    merged: true,
                    message: format!("Merged {} into {into}.{note}", task.branch),
                    into_branch: into,
                    conflicts: Vec::new(),
                })
            }
            Err(conflicts) => Ok(MergeResultDto {
                merged: false,
                message: format!(
                    "Merging {} into {into} conflicts in {} files; nothing was changed.",
                    task.branch,
                    conflicts.len()
                ),
                into_branch: into,
                conflicts,
            }),
        }
    }

    /// Refresh process states and snapshot every task, newest first.
    pub fn snapshots(&self) -> Result<Vec<TaskSnapshot>> {
        let mut inner = self.lock();
        self.reap(&mut inner)?;
        let mut out: Vec<TaskSnapshot> = inner.tasks.iter().cloned().map(TaskSnapshot).collect();
        out.sort_by_key(|t| std::cmp::Reverse(t.0.created_at_ms));
        Ok(out)
    }

    /// Refresh process states and snapshot one task.
    pub fn snapshot(&self, id: &str) -> Result<TaskSnapshot> {
        let mut inner = self.lock();
        self.reap(&mut inner)?;
        inner.task(id).cloned().map(TaskSnapshot)
    }

    /// Convenience for callers that do not care about lock scope (tests, CLI).
    pub fn list_tasks(&self) -> Result<Vec<TaskDto>> {
        Ok(self.snapshots()?.iter().map(describe).collect())
    }

    /// Convenience: one task with git-derived status.
    pub fn task(&self, id: &str) -> Result<TaskDto> {
        self.snapshot(id).map(|s| describe(&s))
    }

    /// Stop a running agent and everything it started. No-op otherwise.
    pub fn stop_task(&self, id: &str) -> Result<TaskSnapshot> {
        let target = {
            let mut inner = self.lock();
            self.reap(&mut inner)?;
            let index = inner.index(id)?;
            if inner.tasks[index].state != TaskState::Running {
                return Ok(TaskSnapshot(inner.tasks[index].clone()));
            }
            let child = inner.children.remove(id);
            let task = &mut inner.tasks[index];
            task.state = TaskState::Stopped;
            let target = (task.pid, task.agent, child, inner.programs.clone());
            self.save(&inner)?;
            target
        };
        // Waiting for the process group happens without the lock.
        let (pid, agent, mut child, programs) = target;
        kill_processes(pid, agent, child.as_mut(), &programs);
        self.snapshot(id)
    }

    /// Stop the agent, remove the worktree and its branch, and forget the task.
    /// Uncommitted and unmerged work in that worktree is lost.
    pub fn discard_task(&self, id: &str) -> Result<()> {
        let owned = self.root.join("worktrees");
        let (task, mut child, programs, _busy) = {
            let mut inner = self.lock();
            let task = inner.task(id)?.clone();
            // Records are data on disk; never trust them to point somewhere safe.
            if !is_strictly_inside(&task.worktree_path, &owned)
                || task.worktree_path.file_name().and_then(|n| n.to_str()) != Some(task.id.as_str())
            {
                return Err(Error::Invalid(
                    "refusing to remove a worktree outside Lore's directory".into(),
                ));
            }
            if !task.branch.starts_with(BRANCH_PREFIX) || task.branch.len() <= BRANCH_PREFIX.len() {
                return Err(Error::Invalid(
                    "refusing to delete a branch Lore did not create".into(),
                ));
            }
            let busy = self.mark_busy(&mut inner, id)?;
            let child = inner.children.remove(id);
            (task, child, inner.programs.clone(), busy)
        };

        kill_processes(task.pid, task.agent, child.as_mut(), &programs);
        let repo_exists = task.repo_path.is_dir();
        let removed_by_git =
            repo_exists && git::remove_worktree(&task.repo_path, &task.worktree_path).is_ok();
        if !removed_by_git && task.worktree_path.exists() {
            // Git no longer knows this worktree (or the repo is gone). Delete the
            // directory ourselves, but only after resolving symlinks.
            let real = fs::canonicalize(&task.worktree_path)?;
            let real_owned = fs::canonicalize(&owned)?;
            if !is_strictly_inside(&real, &real_owned) {
                return Err(Error::Invalid(
                    "refusing to remove a worktree outside Lore's directory".into(),
                ));
            }
            fs::remove_dir_all(&real)?;
        }
        if repo_exists {
            let _ = git::prune_worktrees(&task.repo_path);
            git::delete_branch(&task.repo_path, &task.branch)?;
        }
        if is_strictly_inside(&task.log_path, &self.root.join("logs")) {
            let _ = fs::remove_file(&task.log_path);
        }
        let mut inner = self.lock();
        if let Ok(index) = inner.index(id) {
            inner.tasks.remove(index);
        }
        self.save(&inner)
    }

    /// Stop every running agent (called when the app quits). No git work.
    pub fn shutdown(&self) {
        let mut inner = self.lock();
        let running: Vec<(Option<u32>, Option<Child>)> = inner
            .tasks
            .iter()
            .filter(|t| t.state == TaskState::Running)
            .map(|t| t.id.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .map(|id| {
                let pid = inner.task(&id).ok().and_then(|t| t.pid);
                (pid, inner.children.remove(&id))
            })
            .collect();
        for task in inner
            .tasks
            .iter_mut()
            .filter(|t| t.state == TaskState::Running)
        {
            task.state = TaskState::Stopped;
        }
        let _ = self.save(&inner);
        drop(inner);
        // Signal every group first, then wait once, so quitting with many
        // agents takes one grace period rather than one per agent.
        for (pid, _) in &running {
            if let Some(pid) = pid {
                process::signal_group(*pid, "TERM");
            }
        }
        let deadline = Instant::now() + LINGER_GRACE;
        let mut running = running;
        while Instant::now() < deadline
            && running
                .iter()
                .any(|(pid, _)| pid.is_some_and(process::group_alive))
        {
            for (_, child) in &mut running {
                if let Some(c) = child {
                    let _ = c.try_wait();
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        for (pid, child) in &mut running {
            if let Some(pid) = pid {
                if process::group_alive(*pid) {
                    process::signal_group(*pid, "KILL");
                }
            }
            if let Some(c) = child {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    /// Record the exit of any finished agents. Helpers left in a finished
    /// agent's process group get SIGTERM now and SIGKILL after a grace period;
    /// until they are gone the task counts as not idle.
    fn reap(&self, inner: &mut Inner) -> Result<()> {
        let now = Instant::now();
        inner.lingering.retain(|&pgid, deadline| {
            if !process::group_alive(pgid) {
                return false;
            }
            if now >= *deadline {
                process::signal_group(pgid, "KILL");
            }
            true
        });

        let mut done = Vec::new();
        for (id, child) in &mut inner.children {
            if let Ok(Some(status)) = child.try_wait() {
                done.push((id.clone(), status, child.id()));
            }
        }
        if done.is_empty() {
            return Ok(());
        }
        for (id, status, pid) in done {
            inner.children.remove(&id);
            if process::group_alive(pid) {
                process::signal_group(pid, "TERM");
                inner.lingering.insert(pid, now + LINGER_GRACE);
            }
            if let Some(task) = inner.tasks.iter_mut().find(|t| t.id == id) {
                task.exit_code = status.code().map(i64::from);
                task.state = if status.success() {
                    TaskState::Finished
                } else {
                    TaskState::Failed
                };
            }
        }
        self.save(inner)
    }

    fn save(&self, inner: &Inner) -> Result<()> {
        let tmp = self.root.join(format!("{STORE_FILE}.tmp"));
        {
            let file = File::create(&tmp)?;
            serde_json::to_writer_pretty(&file, &inner.tasks)?;
            file.sync_all()?;
        }
        fs::rename(tmp, self.root.join(STORE_FILE))?;
        Ok(())
    }
}

impl Inner {
    fn index(&self, id: &str) -> Result<usize> {
        self.tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or(Error::NotFound)
    }

    fn task(&self, id: &str) -> Result<&TaskRecord> {
        self.tasks
            .iter()
            .find(|t| t.id == id)
            .ok_or(Error::NotFound)
    }
}

/// The task is not running and nothing it started is still exiting.
fn ensure_idle(inner: &Inner, task: &TaskRecord) -> Result<()> {
    if task.state == TaskState::Running {
        return Err(Error::Invalid("the agent is still running".into()));
    }
    if task
        .pid
        .is_some_and(|pid| inner.lingering.contains_key(&pid) && process::group_alive(pid))
    {
        return Err(Error::Invalid(
            "the agent's helper processes are still exiting; try again in a moment".into(),
        ));
    }
    Ok(())
}

/// Stop a task's process group. Signals a stored pid only when we own the child
/// or the pid still runs the agent program (guards against pid reuse).
fn kill_processes(
    pid: Option<u32>,
    agent: TaskAgent,
    child: Option<&mut Child>,
    programs: &AgentPrograms,
) {
    match (pid, child) {
        (Some(pid), Some(child)) => process::stop_group(pid, Some(child)),
        (Some(pid), None) => {
            if process::group_alive(pid) && process::runs_program(pid, program_for(programs, agent))
            {
                process::stop_group(pid, None);
            }
        }
        (None, Some(child)) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        (None, None) => {}
    }
}

/// Start one agent run for `task`, appending its output to the task log.
fn spawn(
    programs: &AgentPrograms,
    task: &TaskRecord,
    prompt: &str,
    resume: Option<&str>,
) -> Result<Child> {
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&task.log_path)?;
    if task.runs > 1 {
        writeln!(
            log,
            "{} run {} ({}) ---",
            agent::RUN_MARKER,
            task.runs,
            agent_name(task.agent)
        )?;
    }
    let err = log.try_clone()?;
    let child = agent::command(
        programs,
        agent::Launch {
            agent: task.agent,
            permission: task.permission,
            worktree: &task.worktree_path,
            prompt,
            resume_session: resume,
        },
    )
    .process_group(0)
    .stdin(Stdio::null())
    .stdout(Stdio::from(log))
    .stderr(Stdio::from(err))
    .spawn()?;
    Ok(child)
}

/// Full status for a task, including git-derived fields. Runs git subprocesses,
/// so call it without holding the orchestrator.
pub fn describe(snapshot: &TaskSnapshot) -> TaskDto {
    let t = &snapshot.0;
    let exists = t.worktree_path.is_dir();
    let changed = if exists {
        git::changed_files(&t.worktree_path, &t.base_commit, MAX_CHANGED_FILES)
    } else {
        (Vec::new(), 0)
    };
    TaskDto {
        id: t.id.clone(),
        title: t.title.clone(),
        prompt: t.prompt.clone(),
        agent: t.agent,
        state: t.state,
        repo_path: t.repo_path.display().to_string(),
        worktree_path: t.worktree_path.display().to_string(),
        branch: t.branch.clone(),
        base_commit: t.base_commit.clone(),
        created_at_ms: t.created_at_ms,
        exit_code: t.exit_code,
        commits_ahead: if exists {
            git::commits_ahead(&t.worktree_path, &t.base_commit)
        } else {
            0
        },
        changed_files: changed.0,
        changed_files_total: Some(i64::try_from(changed.1).unwrap_or(i64::MAX)),
        last_activity: agent::last_activity(&t.log_path),
        permission: Some(t.permission),
        runs: Some(i64::from(t.runs)),
        uncommitted_count: Some(if exists {
            i64::try_from(git::uncommitted(&t.worktree_path).len()).unwrap_or(i64::MAX)
        } else {
            0
        }),
        attention: if t.state == TaskState::Running {
            None
        } else {
            agent::attention(&t.log_path)
        },
        overlaps: None,
        repo_branch: git::current_branch(&t.repo_path).ok(),
        merged_into: t.merged_into.clone(),
    }
}

/// Mark tasks in the same repository that changed the same files (unmerged
/// tasks only). Call on the full list returned by `describe`.
pub fn annotate_overlaps(tasks: &mut [TaskDto]) {
    let files: Vec<std::collections::HashSet<&str>> = tasks
        .iter()
        .map(|t| t.changed_files.iter().map(String::as_str).collect())
        .collect();
    let mut result: Vec<Vec<TaskOverlapDto>> = vec![Vec::new(); tasks.len()];
    for i in 0..tasks.len() {
        for j in (i + 1)..tasks.len() {
            let (a, b) = (&tasks[i], &tasks[j]);
            if a.repo_path != b.repo_path || a.merged_into.is_some() || b.merged_into.is_some() {
                continue;
            }
            let mut shared: Vec<String> = files[i]
                .intersection(&files[j])
                .map(|s| s.to_string())
                .collect();
            if shared.is_empty() {
                continue;
            }
            shared.sort();
            result[i].push(TaskOverlapDto {
                task_id: b.id.clone(),
                title: b.title.clone(),
                files: shared.clone(),
            });
            result[j].push(TaskOverlapDto {
                task_id: a.id.clone(),
                title: a.title.clone(),
                files: shared,
            });
        }
    }
    for (task, overlaps) in tasks.iter_mut().zip(result) {
        task.overlaps = Some(overlaps);
    }
}

fn agent_name(agent: TaskAgent) -> &'static str {
    match agent {
        TaskAgent::ClaudeCode => "Claude Code",
        TaskAgent::Codex => "Codex",
    }
}

/// Context for an agent picking up a task it has no session for.
fn handoff_brief(task: &TaskRecord, prompt: &str) -> String {
    let wt = &task.worktree_path;
    let commits = git::log_oneline(wt, &task.base_commit, 30);
    let (changed, _) = git::changed_files(wt, &task.base_commit, 100);
    let uncommitted = git::uncommitted(wt);
    let recent: Vec<String> = agent::activity(&task.log_path, 40)
        .into_iter()
        .filter(|e| {
            matches!(
                e.kind,
                ActivityKind::Message | ActivityKind::Result | ActivityKind::Error
            )
        })
        .map(|e| {
            format!(
                "- [{:?}] {}",
                e.kind,
                e.text.chars().take(400).collect::<String>()
            )
        })
        .collect();
    let list = |items: &[String]| {
        if items.is_empty() {
            "(none)".to_string()
        } else {
            items
                .iter()
                .map(|i| format!("- {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    format!(
        "You are continuing a task another agent session started in this git worktree \
         (branch {branch}). Inspect the code yourself before relying on this summary.\n\n\
         ## Original task: {title}\n{original}\n\n\
         ## Commits so far\n{commits}\n\n\
         ## Files changed since the task began\n{changed}\n\n\
         ## Uncommitted paths\n{uncommitted}\n\n\
         ## Recent notes from the previous agent\n{recent}\n\n\
         ## What to do now\n{prompt}\n",
        branch = task.branch,
        title = task.title,
        original = task.prompt,
        commits = list(&commits),
        changed = list(&changed),
        uncommitted = list(&uncommitted),
        recent = if recent.is_empty() {
            "(none)".to_string()
        } else {
            recent.join("\n")
        },
    )
}

const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

/// Unified diff of the task's worktree against its base commit. Runs git; call
/// it without holding the orchestrator.
pub fn diff(snapshot: &TaskSnapshot) -> Result<TaskDiffDto> {
    let t = &snapshot.0;
    if !t.worktree_path.is_dir() {
        return Ok(TaskDiffDto {
            text: String::new(),
            truncated: false,
        });
    }
    let (text, truncated) = git::diff_against(&t.worktree_path, &t.base_commit, MAX_DIFF_BYTES)?;
    Ok(TaskDiffDto { text, truncated })
}

/// The task agent's recent activity timeline (see [`agent::activity`]).
pub fn activity(snapshot: &TaskSnapshot, max_events: usize) -> Vec<ActivityDto> {
    agent::activity(&snapshot.0.log_path, max_events)
}

fn program_for(programs: &AgentPrograms, agent: TaskAgent) -> &Path {
    match agent {
        TaskAgent::ClaudeCode => &programs.claude_code,
        TaskAgent::Codex => &programs.codex,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Unique within this process and practically unique across restarts.
fn new_id(now_ms: i64) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mix = nanos ^ std::process::id().rotate_left(16) ^ COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("t{now_ms:x}{:08x}", mix)
}

/// Lowercase ASCII slug for branch names; never empty.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "task".into()
    } else {
        out
    }
}

/// `path` is a descendant of `dir` after lexical normalization (no `..`).
fn is_strictly_inside(path: &Path, dir: &Path) -> bool {
    use std::path::Component;
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    path.starts_with(dir) && path != dir
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_branch_safe() {
        assert_eq!(slug("Fix the Stripe webhook!!"), "fix-the-stripe-webhook");
        assert_eq!(slug("   ***  "), "task");
        assert!(slug(&"x".repeat(100)).len() <= 40);
    }

    #[test]
    fn inside_check_rejects_escape() {
        let dir = Path::new("/a/worktrees");
        assert!(is_strictly_inside(Path::new("/a/worktrees/t1"), dir));
        assert!(!is_strictly_inside(Path::new("/a/worktrees"), dir));
        assert!(!is_strictly_inside(Path::new("/a/worktrees/../repo"), dir));
        assert!(!is_strictly_inside(Path::new("/a/other"), dir));
    }

    #[test]
    fn ids_do_not_collide_in_a_burst() {
        let ids: std::collections::HashSet<_> = (0..1000).map(|_| new_id(1)).collect();
        assert_eq!(ids.len(), 1000);
    }
}
