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
    ActivityDto, ActivityKind, ContinueTaskRequest, CreateTaskRequest, DecisionDto, MergeQueueDto,
    MergeQueueItemDto, MergeResultDto, RepoSettingsDto, TaskAgent, TaskDiffDto, TaskDto,
    TaskEffort, TaskOverlapDto, TaskPermission, TaskState,
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
const DECISIONS_FILE: &str = "decisions.jsonl";
const REPOS_FILE: &str = "repos.json";
const QUEUES_FILE: &str = "queues.json";
const QUEUE_CANCEL_WHY: &str = "Lore quit while this merge queue was running. Test processes were stopped. Remaining steps were not merged.";
const QUEUE_INTERRUPT_WHY: &str = "Lore exited while this merge queue was running, so it was interrupted. Test processes were stopped. Remaining steps were not merged.";
const MAX_TEST_COMMAND: usize = 2_000;
/// How long a merge-queue test command may run before it is stopped.
const DEFAULT_TEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_DECISIONS: usize = 2_000;
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
    /// Model alias/id for `--model`, when the user picked one.
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<TaskEffort>,
    /// Runs in this worktree: the first launch plus each continuation.
    #[serde(default = "one")]
    runs: u32,
    /// Branch of the primary checkout this task was merged into.
    #[serde(default)]
    merged_into: Option<String>,
    /// Repository-relative files/folders this task owns.
    #[serde(default)]
    claims: Vec<String>,
    #[serde(default = "yes")]
    auto_handoff: bool,
    /// `runs` value at which an automatic handoff already happened.
    #[serde(default)]
    handoff_run: Option<u32>,
    /// Automatic handoffs since the user last continued the task by hand. One
    /// is allowed; if the other agent is also out of quota, the task waits.
    #[serde(default)]
    auto_handoffs: u32,
}

fn yes() -> bool {
    true
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
    /// Repositories with a merge in progress; git allows one at a time.
    merging_repos: HashSet<PathBuf>,
    /// Set by `shutdown`; no new agent may start afterwards.
    shutting_down: bool,
    /// Merge queues by repository, including finished ones until replaced.
    queues: HashMap<PathBuf, MergeQueueDto>,
    /// Repositories whose running queue was asked to stop.
    cancel_queues: HashSet<PathBuf>,
    /// Process groups of merge-queue test commands that are running, keyed by
    /// repository so a relaunch can stop an orphaned test.
    queue_tests: HashMap<PathBuf, u32>,
    test_timeout: Duration,
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

/// Marks a repository's merge queue finished when its runner ends, however it ends.
struct QueueFinish<'a> {
    orchestrator: &'a Orchestrator,
    repo: PathBuf,
}

impl Drop for QueueFinish<'_> {
    fn drop(&mut self) {
        let mut inner = self.orchestrator.lock();
        inner.cancel_queues.remove(&self.repo);
        if let Some(q) = inner.queues.get_mut(&self.repo) {
            q.running = false;
            for item in &mut q.items {
                if matches!(
                    item.status.as_str(),
                    "pending" | "updating" | "testing" | "merging"
                ) {
                    item.status = "cancelled".into();
                }
            }
        }
        let _ = self.orchestrator.save_queues(&inner);
    }
}

/// Releases a repository's merge slot when a merge ends, however it ends.
struct RepoMerge<'a> {
    orchestrator: &'a Orchestrator,
    repo: PathBuf,
}

impl Drop for RepoMerge<'_> {
    fn drop(&mut self) {
        self.orchestrator.lock().merging_repos.remove(&self.repo);
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

        let mut queue_warning = None;
        let (mut queues, mut leftover_tests) = match load_queues_file(&root) {
            Ok(loaded) => loaded,
            Err(msg) => {
                queue_warning = Some(msg);
                (HashMap::new(), HashMap::new())
            }
        };
        let mut queue_events = Vec::new();
        for (repo, queue) in &mut queues {
            if let Some(pgid) = leftover_tests.remove(repo) {
                if process::group_alive(pgid) {
                    process::stop_group(pgid, None);
                }
            }
            if queue.running {
                mark_queue_stopped(queue, "interrupted", QUEUE_INTERRUPT_WHY);
                queue_events.push(queue.clone());
            }
        }
        if load_warning.is_none() {
            load_warning = queue_warning;
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
                merging_repos: HashSet::new(),
                shutting_down: false,
                queues,
                cancel_queues: HashSet::new(),
                queue_tests: HashMap::new(),
                test_timeout: DEFAULT_TEST_TIMEOUT,
                load_warning,
            }),
            _instance: instance,
        };
        if changed {
            let inner = orchestrator.lock();
            orchestrator.save(&inner)?;
        }
        if !queue_events.is_empty() {
            let inner = orchestrator.lock();
            let _ = orchestrator.save_queues(&inner);
            drop(inner);
            for queue in &queue_events {
                orchestrator.record_queue_event(queue, "queue_interrupted", QUEUE_INTERRUPT_WHY);
            }
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

        let claims = normalize_claims(req.claims.as_deref().unwrap_or(&[]))?;
        let model = agent::parse_model(req.model.as_deref())?;

        // Git work, without the lock.
        let repo = git::toplevel(&requested).map_err(workspace::not_a_git_repository)?;
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
            model,
            effort: req.effort,
            runs: 1,
            merged_into: None,
            claims: claims.clone(),
            auto_handoff: req.auto_handoff.unwrap_or(true),
            handoff_run: None,
            auto_handoffs: 0,
        };

        let mut inner = self.lock();
        if inner.shutting_down {
            drop(inner);
            let _ = git::remove_worktree(&record.repo_path, &record.worktree_path);
            let _ = git::delete_branch(&record.repo_path, &record.branch);
            return Err(Error::Invalid("Lore is quitting".into()));
        }
        let opening = format!(
            "{}{}",
            coordination_preamble(&inner.tasks, &record),
            record.prompt
        );
        match spawn(&inner.programs, &record, &opening, None) {
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
        inner.tasks.push(record.clone());
        if let Err(e) = self.save(&inner) {
            // Without a record nothing could ever clean this up, so roll back.
            drop(inner);
            let _ = self.discard_task(&id);
            return Err(e);
        }
        let created = inner.tasks.last().cloned().ok_or(Error::NotFound)?;
        drop(inner);
        self.record_decision(
            &created,
            "created",
            &format!(
                "{} on {} (owns: {})",
                agent_name(created.agent),
                created.branch,
                if created.claims.is_empty() {
                    "nothing declared".to_string()
                } else {
                    created.claims.join(", ")
                }
            ),
        );
        Ok(TaskSnapshot(created))
    }

    /// Send more work to a task that is not running, in the same worktree.
    ///
    /// Keeping the agent resumes its own session when it supports that (Claude);
    /// otherwise, including every switch of agent (a handoff), the new run gets a
    /// brief built from the task's commits, changes, and recent activity.
    pub fn continue_task(&self, req: &ContinueTaskRequest) -> Result<TaskSnapshot> {
        self.continue_task_inner(req, false)
    }

    fn continue_task_inner(
        &self,
        req: &ContinueTaskRequest,
        automatic: bool,
    ) -> Result<TaskSnapshot> {
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
        let next_model = match &req.model {
            Some(raw) => agent::parse_model(Some(raw))?,
            None => task.model.clone(),
        };
        let next_effort = req.effort.or(task.effort);
        let same_setup =
            new_agent == task.agent && next_model == task.model && next_effort == task.effort;
        let session = (same_setup && new_agent == TaskAgent::ClaudeCode)
            .then(|| agent::claude_session_id(&task.log_path))
            .flatten();
        let full_prompt = match session {
            Some(_) => prompt.to_string(),
            None => handoff_brief(
                &task,
                prompt,
                &self.decisions(Some(&task.repo_path.display().to_string()), 15),
            ),
        };

        let mut inner = self.lock();
        if inner.shutting_down {
            return Err(Error::Invalid("Lore is quitting".into()));
        }
        let index = inner.index(&req.id)?;
        let previous = inner.tasks[index].clone();
        let mut record = previous.clone();
        record.agent = new_agent;
        record.model = next_model;
        record.effort = next_effort;
        record.runs += 1;
        if !automatic {
            record.auto_handoffs = 0;
        }
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
        let updated = inner.tasks[index].clone();
        drop(inner);
        self.record_decision(
            &updated,
            if req.agent.is_some_and(|a| a != previous.agent) {
                "handoff"
            } else {
                "continued"
            },
            &format!("run {} with {}", updated.runs, agent_name(updated.agent)),
        );
        Ok(TaskSnapshot(updated))
    }

    /// Commit every uncommitted change in the task's worktree.
    pub fn commit_task(&self, id: &str, message: &str) -> Result<TaskSnapshot> {
        self.commit_task_forced(id, message, false)
    }

    /// As [`Orchestrator::commit_task`], but `force` commits even when the task
    /// changed files another unmerged task claims.
    pub fn commit_task_forced(&self, id: &str, message: &str, force: bool) -> Result<TaskSnapshot> {
        let (task, others, _busy) = {
            let mut inner = self.lock();
            self.reap(&mut inner)?;
            let task = inner.task(id)?.clone();
            ensure_idle(&inner, &task)?;
            let busy = self.mark_busy(&mut inner, id)?;
            let others: Vec<(String, Vec<String>)> = inner
                .tasks
                .iter()
                .filter(|t| {
                    t.id != task.id && t.repo_path == task.repo_path && t.merged_into.is_none()
                })
                .map(|t| (t.title.clone(), t.claims.clone()))
                .collect();
            (task, others, busy)
        };
        if !force {
            let (changed, _) =
                git::changed_files(&task.worktree_path, &task.base_commit, usize::MAX);
            let mut trespass: Vec<String> = Vec::new();
            for (title, claims) in &others {
                for file in &changed {
                    if claims.iter().any(|c| claim_covers(c, file)) {
                        trespass.push(format!("{file} (owned by \"{title}\")"));
                    }
                }
            }
            if !trespass.is_empty() {
                trespass.sort();
                trespass.dedup();
                trespass.truncate(20);
                return Err(Error::Invalid(format!(
                    "this task changed files another agent owns: {}. Review them, then commit \
                     again with force if that is intended.",
                    trespass.join(", ")
                )));
            }
        }
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
        self.record_decision(&task, "committed", &message);
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
            if !inner.merging_repos.insert(task.repo_path.clone()) {
                return Err(Error::Invalid(
                    "another merge into this repository is in progress".into(),
                ));
            }
            let busy = self.mark_busy(&mut inner, id);
            let busy = match busy {
                Ok(b) => b,
                Err(e) => {
                    inner.merging_repos.remove(&task.repo_path);
                    return Err(e);
                }
            };
            (task, busy)
        };
        let _merging = RepoMerge {
            orchestrator: self,
            repo: task.repo_path.clone(),
        };
        self.merge_prepared(&task)
    }

    /// The merge itself, for a caller that already holds the task's busy marker
    /// and the repository's merge slot.
    fn merge_prepared(&self, task: &TaskRecord) -> Result<MergeResultDto> {
        let id = task.id.as_str();
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
                self.record_decision(task, "merged", &format!("into {into}"));
                Ok(MergeResultDto {
                    merged: true,
                    message: format!("Merged {} into {into}.{note}", task.branch),
                    into_branch: into,
                    conflicts: Vec::new(),
                })
            }
            Err(conflicts) => {
                self.record_decision(
                    task,
                    "merge_conflict",
                    &format!("into {into}: {}", conflicts.join(", ")),
                );
                Ok(MergeResultDto {
                    merged: false,
                    message: format!(
                        "Merging {} into {into} conflicts in {} files; nothing was changed.",
                        task.branch,
                        conflicts.len()
                    ),
                    into_branch: into,
                    conflicts,
                })
            }
        }
    }

    fn load_repo_settings(&self) -> HashMap<String, RepoSettingsDto> {
        fs::read(self.root.join(REPOS_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Settings Lore keeps for a repository (any path inside it).
    pub fn repo_settings(&self, repo_path: &str) -> Result<RepoSettingsDto> {
        let top = workspace::repository_root(repo_path)?;
        let key = top.display().to_string();
        Ok(self
            .load_repo_settings()
            .remove(&key)
            .unwrap_or(RepoSettingsDto {
                repo_path: key,
                test_command: None,
            }))
    }

    /// Set or clear the command the merge queue runs before merging a task.
    pub fn set_repo_test_command(
        &self,
        repo_path: &str,
        command: Option<&str>,
    ) -> Result<RepoSettingsDto> {
        let top = workspace::repository_root(repo_path)?;
        let key = top.display().to_string();
        let command = command.map(str::trim).filter(|c| !c.is_empty());
        if let Some(c) = command {
            if c.len() > MAX_TEST_COMMAND || c.contains('\0') {
                return Err(Error::Invalid(format!(
                    "test command must be under {MAX_TEST_COMMAND} characters"
                )));
            }
        }
        // One writer at a time for the settings file.
        let _guard = self.lock();
        let mut all = self.load_repo_settings();
        let settings = RepoSettingsDto {
            repo_path: key.clone(),
            test_command: command.map(str::to_string),
        };
        all.insert(key, settings.clone());
        let tmp = self.root.join(format!("{REPOS_FILE}.tmp"));
        fs::write(&tmp, serde_json::to_vec_pretty(&all)?)?;
        fs::rename(tmp, self.root.join(REPOS_FILE))?;
        Ok(settings)
    }

    /// Override the merge-queue test timeout (tests use a short one).
    pub fn set_test_timeout(&self, timeout: Duration) {
        self.lock().test_timeout = timeout;
    }

    /// The latest merge queue for a repository, including one restored after relaunch.
    pub fn merge_queue(&self, repo_path: &str) -> Option<MergeQueueDto> {
        let top = workspace::repository_root(repo_path).ok()?;
        self.lock().queues.get(&top).cloned()
    }

    /// Ask a running queue to stop after (or during) its current step.
    pub fn cancel_merge_queue(&self, repo_path: &str) -> Result<()> {
        let top = workspace::repository_root(repo_path)?;
        let mut inner = self.lock();
        if inner.queues.get(&top).is_some_and(|q| q.running) {
            inner.cancel_queues.insert(top);
        }
        Ok(())
    }

    /// Validate a queue and register it as running, holding the repository's
    /// merge slot. Call [`Orchestrator::run_merge_queue`] next (on a thread).
    pub fn start_merge_queue(&self, repo_path: &str, task_ids: &[String]) -> Result<MergeQueueDto> {
        if task_ids.is_empty() {
            return Err(Error::Invalid("choose at least one task to merge".into()));
        }
        let top = workspace::repository_root(repo_path)?;
        let test_command = self.repo_settings(&top.display().to_string())?.test_command;
        let mut inner = self.lock();
        self.reap(&mut inner)?;
        let mut items = Vec::new();
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for id in task_ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            let task = inner.task(id)?;
            let same_repo = fs::canonicalize(&task.repo_path).is_ok_and(|p| p == top);
            if !same_repo {
                return Err(Error::Invalid(format!(
                    "\"{}\" belongs to a different repository",
                    task.title
                )));
            }
            if task.merged_into.is_some() {
                return Err(Error::Invalid(format!(
                    "\"{}\" is already merged",
                    task.title
                )));
            }
            ensure_idle(&inner, task)?;
            items.push(MergeQueueItemDto {
                task_id: id.clone(),
                title: task.title.clone(),
                status: "pending".into(),
                detail: None,
            });
            selected.push(task.clone());
        }
        if let Some(msg) = overlapping_claim_error(&selected) {
            return Err(Error::Invalid(msg));
        }
        if inner.queues.get(&top).is_some_and(|q| q.running) {
            return Err(Error::Invalid(
                "a merge queue is already running for this repository".into(),
            ));
        }
        if !inner.merging_repos.insert(top.clone()) {
            return Err(Error::Invalid(
                "another merge into this repository is in progress".into(),
            ));
        }
        inner.cancel_queues.remove(&top);
        let queue = MergeQueueDto {
            repo_path: top.display().to_string(),
            running: true,
            test_command,
            items,
        };
        inner.queues.insert(top, queue.clone());
        self.save_queues(&inner)?;
        Ok(queue)
    }

    /// Run a started queue to completion: for each task in order, bring its
    /// branch up to date with the target branch, run the repository's test
    /// command in the task's worktree, then merge. Stops at the first failure;
    /// later tasks are marked skipped. Blocking.
    pub fn run_merge_queue(&self, repo_path: &str) -> MergeQueueDto {
        let top =
            workspace::repository_root(repo_path).unwrap_or_else(|_| PathBuf::from(repo_path));
        let _slot = RepoMerge {
            orchestrator: self,
            repo: top.clone(),
        };
        // Clears `running` however this function ends, including a panic.
        let _finish = QueueFinish {
            orchestrator: self,
            repo: top.clone(),
        };
        let (ids, test_command, timeout) = {
            let inner = self.lock();
            let Some(q) = inner.queues.get(&top) else {
                return MergeQueueDto {
                    repo_path: top.display().to_string(),
                    running: false,
                    test_command: None,
                    items: Vec::new(),
                };
            };
            (
                q.items
                    .iter()
                    .map(|i| i.task_id.clone())
                    .collect::<Vec<_>>(),
                q.test_command.clone(),
                inner.test_timeout,
            )
        };

        let mut stopped: Option<&str> = None;
        for (index, id) in ids.iter().enumerate() {
            if self.queue_cancelled(&top) {
                self.set_queue_item(&top, index, "cancelled", None);
                stopped = Some("the queue was cancelled");
                continue;
            }
            if let Some(why) = stopped {
                self.set_queue_item(&top, index, "skipped", Some(why));
                continue;
            }
            match self.run_queue_item(&top, index, id, test_command.as_deref(), timeout) {
                Ok(()) => {}
                Err(reason) => {
                    let cancelled = self.queue_cancelled(&top);
                    self.set_queue_item(
                        &top,
                        index,
                        if cancelled { "cancelled" } else { "failed" },
                        Some(&reason),
                    );
                    stopped = Some(if cancelled {
                        "the queue was cancelled"
                    } else {
                        "an earlier task failed"
                    });
                }
            }
        }

        let mut inner = self.lock();
        inner.cancel_queues.remove(&top);
        let queue = match inner.queues.get_mut(&top) {
            Some(q) => {
                q.running = false;
                q.clone()
            }
            None => MergeQueueDto {
                repo_path: top.display().to_string(),
                running: false,
                test_command,
                items: Vec::new(),
            },
        };
        let _ = self.save_queues(&inner);
        queue
    }

    /// Release a started queue whose runner could not be started.
    pub fn abandon_merge_queue(&self, repo_path: &str) {
        let top =
            workspace::repository_root(repo_path).unwrap_or_else(|_| PathBuf::from(repo_path));
        drop(QueueFinish {
            orchestrator: self,
            repo: top.clone(),
        });
        self.lock().merging_repos.remove(&top);
    }

    fn queue_cancelled(&self, repo: &Path) -> bool {
        self.lock().cancel_queues.contains(repo)
    }

    fn set_queue_item(&self, repo: &Path, index: usize, status: &str, detail: Option<&str>) {
        let mut inner = self.lock();
        if let Some(item) = inner
            .queues
            .get_mut(repo)
            .and_then(|q| q.items.get_mut(index))
        {
            // Quit/relaunch already recorded a durable outcome; don't replace it
            // with the test command's SIGTERM as a failure.
            if matches!(item.status.as_str(), "cancelled" | "interrupted") {
                let _ = self.save_queues(&inner);
                return;
            }
            item.status = status.to_string();
            item.detail = detail.map(|d| d.chars().take(2_000).collect());
        }
        let _ = self.save_queues(&inner);
    }

    /// One queue step. Returns a human-readable reason on failure.
    fn run_queue_item(
        &self,
        repo: &Path,
        index: usize,
        id: &str,
        test_command: Option<&str>,
        timeout: Duration,
    ) -> std::result::Result<(), String> {
        let (task, _busy) = {
            let mut inner = self.lock();
            let _ = self.reap(&mut inner);
            let task = inner.task(id).map_err(|e| e.to_string())?.clone();
            ensure_idle(&inner, &task).map_err(|e| e.to_string())?;
            if task.merged_into.is_some() {
                return Err("already merged".into());
            }
            let busy = self.mark_busy(&mut inner, id).map_err(|e| e.to_string())?;
            (task, busy)
        };
        if !git::uncommitted(&task.worktree_path).is_empty() {
            return Err("the task has uncommitted changes; commit them first".into());
        }
        // Never commit an update onto anything but the task's own branch.
        if git::checked_out_branch(&task.worktree_path).as_deref() != Some(task.branch.as_str()) {
            return Err(format!(
                "the task's worktree is no longer on {}; check out that branch there first",
                task.branch
            ));
        }
        if git::branch_commits_ahead(&task.repo_path, &task.base_commit, &task.branch) == 0 {
            return Err("the task has no commits to merge".into());
        }

        self.set_queue_item(repo, index, "updating", None);
        // Pin the target: the branch and commit tested are the ones merged into.
        let into = git::current_branch(&task.repo_path).map_err(|e| e.to_string())?;
        let target = git::head_commit(&task.repo_path).map_err(|e| e.to_string())?;
        match git::merge_into_worktree(&task.worktree_path, &target) {
            Ok(Ok(())) => {
                // Diffs, changed files, and claims stay about this task's own work.
                let mut inner = self.lock();
                if let Ok(i) = inner.index(id) {
                    inner.tasks[i].base_commit = target.clone();
                    let _ = self.save(&inner);
                }
            }
            Ok(Err(conflicts)) => {
                self.record_decision(
                    &task,
                    "queue_failed",
                    &format!("updating from {into} conflicts: {}", conflicts.join(", ")),
                );
                return Err(format!(
                    "updating from {into} conflicts in {}; resolve it in the worktree or continue the agent",
                    conflicts.join(", ")
                ));
            }
            Err(e) => return Err(format!("could not update from {into}: {e}")),
        }

        let before_tests: HashSet<String> =
            git::uncommitted(&task.worktree_path).into_iter().collect();
        if let Some(command) = test_command {
            self.set_queue_item(repo, index, "testing", Some(command));
            let log = self.root.join("logs").join(format!("{id}.test.log"));
            let repo_key = repo.to_path_buf();
            let outcome = run_test_command(
                &task.worktree_path,
                command,
                &log,
                timeout,
                || self.queue_cancelled(repo),
                |pgid, running| {
                    let mut inner = self.lock();
                    if running {
                        inner.queue_tests.insert(repo_key.clone(), pgid);
                    } else {
                        inner.queue_tests.remove(&repo_key);
                    }
                    let _ = self.save_queues(&inner);
                },
            );
            let _ = git::discard_new_test_artifacts(&task.worktree_path, &before_tests);
            if let Err(reason) = outcome {
                if self.queue_cancelled(repo) {
                    return Err("cancelled".into());
                }
                self.record_decision(&task, "queue_failed", &format!("tests: {reason}"));
                return Err(format!("tests failed: {reason}"));
            }
            let leftover = git::uncommitted(&task.worktree_path);
            if !leftover.is_empty() {
                return Err(format!(
                    "the task has uncommitted changes after tests ({}); commit them first",
                    leftover.join(", ")
                ));
            }
        }

        if self.queue_cancelled(repo) {
            return Err("cancelled before merging".into());
        }
        // The target branch must be the one that was tested.
        let now = git::current_branch(&task.repo_path).map_err(|e| e.to_string())?;
        let head = git::head_commit(&task.repo_path).map_err(|e| e.to_string())?;
        if now != into || head != target {
            return Err(format!(
                "{into} changed while this task was being tested; run the queue again"
            ));
        }
        self.set_queue_item(repo, index, "merging", None);
        let mut updated = task.clone();
        updated.base_commit = target;
        let result = self.merge_prepared(&updated).map_err(|e| e.to_string())?;
        if result.merged {
            self.set_queue_item(repo, index, "merged", Some(&result.message));
            Ok(())
        } else {
            Err(result.message)
        }
    }

    /// Append one entry to the shared decision log. Best-effort: a task must
    /// never fail because its history could not be written.
    fn record_decision(&self, task: &TaskRecord, kind: &str, detail: &str) {
        self.append_decision(DecisionDto {
            at_ms: now_ms(),
            task_id: task.id.clone(),
            task_title: task.title.clone(),
            repo_path: task.repo_path.display().to_string(),
            kind: kind.to_string(),
            detail: detail.chars().take(2_000).collect(),
        });
    }

    fn record_queue_event(&self, queue: &MergeQueueDto, kind: &str, detail: &str) {
        let (task_id, task_title) = queue
            .items
            .first()
            .map(|item| (item.task_id.clone(), item.title.clone()))
            .unwrap_or_else(|| (String::new(), "Merge queue".into()));
        self.append_decision(DecisionDto {
            at_ms: now_ms(),
            task_id,
            task_title,
            repo_path: queue.repo_path.clone(),
            kind: kind.to_string(),
            detail: detail.chars().take(2_000).collect(),
        });
    }

    fn append_decision(&self, entry: DecisionDto) {
        if let (Ok(mut file), Ok(line)) = (
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.root.join(DECISIONS_FILE)),
            serde_json::to_string(&entry),
        ) {
            let _ = writeln!(file, "{line}");
        }
    }

    fn save_queues(&self, inner: &Inner) -> Result<()> {
        let mut map = HashMap::new();
        for (repo, queue) in &inner.queues {
            map.insert(
                repo.display().to_string(),
                PersistentQueue {
                    queue: queue.clone(),
                    test_pgid: inner.queue_tests.get(repo).copied(),
                },
            );
        }
        let tmp = self.root.join(format!("{QUEUES_FILE}.tmp"));
        {
            let file = File::create(&tmp)?;
            serde_json::to_writer_pretty(&file, &map)?;
            file.sync_all()?;
        }
        fs::rename(tmp, self.root.join(QUEUES_FILE))?;
        Ok(())
    }

    /// The most recent decisions, newest first, optionally for one repository.
    pub fn decisions(&self, repo_path: Option<&str>, limit: usize) -> Vec<DecisionDto> {
        let Ok(text) = fs::read_to_string(self.root.join(DECISIONS_FILE)) else {
            return Vec::new();
        };
        let mut out: Vec<DecisionDto> = text
            .lines()
            .rev()
            .filter_map(|l| serde_json::from_str::<DecisionDto>(l).ok())
            .filter(|d| repo_path.is_none_or(|r| d.repo_path == r))
            .take(limit.min(MAX_DECISIONS))
            .collect();
        out.sort_by_key(|d| std::cmp::Reverse(d.at_ms));
        out
    }

    /// Cheap per-task change signals for the UI: state and how much output the
    /// agent has produced. No git, no parsing.
    pub fn change_signatures(&self) -> Vec<(String, TaskState, u64)> {
        let mut inner = self.lock();
        let _ = self.reap(&mut inner);
        inner
            .tasks
            .iter()
            .map(|t| {
                let len = fs::metadata(&t.log_path).map(|m| m.len()).unwrap_or(0);
                (t.id.clone(), t.state, len)
            })
            .collect()
    }

    /// Hand tasks that stopped on a usage limit to the other agent, once per
    /// run. Returns the ids that were handed off.
    pub fn run_auto_handoffs(&self) -> Vec<String> {
        let candidates: Vec<TaskRecord> = {
            let mut inner = self.lock();
            let _ = self.reap(&mut inner);
            inner
                .tasks
                .iter()
                .filter(|t| {
                    t.auto_handoff
                        && t.merged_into.is_none()
                        && matches!(t.state, TaskState::Failed | TaskState::Finished)
                        && t.handoff_run != Some(t.runs)
                        && t.auto_handoffs == 0
                        && !inner.busy.contains(&t.id)
                })
                .cloned()
                .collect()
        };
        let mut handed = Vec::new();
        for task in candidates {
            if !agent::usage_limited(&task.log_path) {
                continue;
            }
            let other = match task.agent {
                TaskAgent::ClaudeCode => TaskAgent::Codex,
                TaskAgent::Codex => TaskAgent::ClaudeCode,
            };
            // Mark the attempt first: a failing handoff must not retry forever.
            {
                let mut inner = self.lock();
                if let Ok(index) = inner.index(&task.id) {
                    inner.tasks[index].handoff_run = Some(task.runs);
                    inner.tasks[index].auto_handoffs += 1;
                    let _ = self.save(&inner);
                }
            }
            let request = ContinueTaskRequest {
                id: task.id.clone(),
                prompt: format!(
                    "The previous agent stopped because it hit its usage limit. \
                     Continue this task from where it left off: {}",
                    task.title
                ),
                agent: Some(other),
                model: None,
                effort: None,
            };
            if self.continue_task_inner(&request, true).is_ok() {
                self.record_decision(
                    &task,
                    "auto_handoff",
                    &format!(
                        "{} hit its usage limit; handed off to {}",
                        agent_name(task.agent),
                        agent_name(other)
                    ),
                );
                handed.push(task.id);
            }
        }
        handed
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
            let saved = self.save(&inner);
            (target, saved)
        };
        // Waiting for the process group happens without the lock. Stop the agent
        // even if saving failed: an untracked agent must never keep running.
        let ((pid, agent, mut child, programs), saved) = target;
        kill_processes(pid, agent, child.as_mut(), &programs);
        saved?;
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
        let mut branch_note = String::new();
        if repo_exists {
            let _ = git::prune_worktrees(&task.repo_path);
            // A branch checked out elsewhere cannot be deleted; the task is still
            // discarded, and the branch is left for the user.
            if let Err(e) = git::delete_branch(&task.repo_path, &task.branch) {
                branch_note = format!(" (branch kept: {e})");
            }
        }
        if is_strictly_inside(&task.log_path, &self.root.join("logs")) {
            let _ = fs::remove_file(&task.log_path);
        }
        let mut inner = self.lock();
        if let Ok(index) = inner.index(id) {
            inner.tasks.remove(index);
        }
        let result = self.save(&inner);
        drop(inner);
        self.record_decision(&task, "discarded", &format!("{}{branch_note}", task.branch));
        result
    }

    /// Stop every running agent (called when the app quits). No git work.
    pub fn shutdown(&self) {
        let mut inner = self.lock();
        inner.shutting_down = true;
        // Running merge queues stop, and so do their test commands.
        let running_queues: Vec<PathBuf> = inner
            .queues
            .iter()
            .filter(|(_, q)| q.running)
            .map(|(repo, _)| repo.clone())
            .collect();
        inner.cancel_queues.extend(running_queues.iter().cloned());
        let mut cancelled_queues = Vec::new();
        for repo in &running_queues {
            if let Some(queue) = inner.queues.get_mut(repo) {
                mark_queue_stopped(queue, "cancelled", QUEUE_CANCEL_WHY);
                cancelled_queues.push(queue.clone());
            }
        }
        let queue_pgids: Vec<u32> = inner.queue_tests.drain().map(|(_, pgid)| pgid).collect();
        for pgid in &queue_pgids {
            process::signal_group(*pgid, "TERM");
        }
        let _ = self.save_queues(&inner);
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
        for queue in &cancelled_queues {
            self.record_queue_event(queue, "queue_cancelled", QUEUE_CANCEL_WHY);
        }
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
            && (running
                .iter()
                .any(|(pid, _)| pid.is_some_and(process::group_alive))
                || queue_pgids.iter().copied().any(process::group_alive))
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
        for pgid in queue_pgids {
            if process::group_alive(pgid) {
                process::signal_group(pgid, "KILL");
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistentQueue {
    #[serde(flatten)]
    queue: MergeQueueDto,
    #[serde(default)]
    test_pgid: Option<u32>,
}

type LoadedQueues = (HashMap<PathBuf, MergeQueueDto>, HashMap<PathBuf, u32>);

fn load_queues_file(root: &Path) -> std::result::Result<LoadedQueues, String> {
    let path = root.join(QUEUES_FILE);
    match fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((HashMap::new(), HashMap::new())),
        Err(e) => Err(format!("The merge queue list could not be read ({e}).")),
        Ok(bytes) => match serde_json::from_slice::<HashMap<String, PersistentQueue>>(&bytes) {
            Ok(map) => {
                let mut queues = HashMap::new();
                let mut tests = HashMap::new();
                for (key, stored) in map {
                    let repo = PathBuf::from(key);
                    if let Some(pgid) = stored.test_pgid {
                        tests.insert(repo.clone(), pgid);
                    }
                    queues.insert(repo, stored.queue);
                }
                Ok((queues, tests))
            }
            Err(e) => {
                let aside = root.join(format!("{QUEUES_FILE}.corrupt-{}", now_ms()));
                let _ = fs::rename(&path, &aside);
                Err(format!(
                    "The merge queue list could not be read ({e}) and was moved to {}.",
                    aside.display()
                ))
            }
        },
    }
}

fn mark_queue_stopped(queue: &mut MergeQueueDto, status: &str, why: &str) {
    queue.running = false;
    for item in &mut queue.items {
        if matches!(
            item.status.as_str(),
            "pending" | "updating" | "testing" | "merging"
        ) {
            item.status = status.to_string();
            item.detail = Some(why.chars().take(2_000).collect());
        }
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

const TEST_LOG_TAIL: u64 = 4_000;

/// Run `command` with `/bin/sh -c` in `worktree`, output to `log`. Stops the
/// whole process group on timeout or when `cancelled` turns true.
fn run_test_command(
    worktree: &Path,
    command: &str,
    log: &Path,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
    track: impl Fn(u32, bool),
) -> std::result::Result<(), String> {
    let out = File::create(log).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(worktree)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|e| format!("could not start the test command: {e}"))?;
    let pgid = child.id();
    track(pgid, true);
    let result = wait_test_command(&mut child, pgid, log, timeout, cancelled);
    track(pgid, false);
    result
}

fn wait_test_command(
    child: &mut Child,
    pgid: u32,
    log: &Path,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
) -> std::result::Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Clean up anything the tests left running.
                if process::group_alive(pgid) {
                    process::stop_group(pgid, None);
                }
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "exit {}; last output:\n{}",
                        status
                            .code()
                            .map_or("signal".to_string(), |c| c.to_string()),
                        log_tail(log)
                    ))
                };
            }
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
        if cancelled() {
            process::stop_group(pgid, Some(child));
            return Err("cancelled".into());
        }
        if Instant::now() >= deadline {
            process::stop_group(pgid, Some(child));
            return Err(format!("timed out after {}s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn log_tail(log: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = File::open(log) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(TEST_LOG_TAIL)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).trim().to_string()
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
    writeln!(log, "{} opening prompt ---", agent::RUN_MARKER)?;
    writeln!(log, "{prompt}")?;
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
            model: task.model.as_deref(),
            effort: task.effort,
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
        last_activity: agent::last_activity(&t.log_path)
            .map(|s| strip_worktree_prefix(&s, &t.worktree_path)),
        permission: Some(t.permission),
        model: t.model.clone(),
        effort: t.effort,
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
        claims: Some(t.claims.clone()),
        claim_conflicts: None,
        auto_handoff: Some(t.auto_handoff),
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
    annotate_claim_conflicts(tasks);
}

/// Mark files a task changed that another unmerged task in the same repository
/// claims as its own.
fn annotate_claim_conflicts(tasks: &mut [TaskDto]) {
    let mut result: Vec<Vec<TaskOverlapDto>> = vec![Vec::new(); tasks.len()];
    for i in 0..tasks.len() {
        for j in 0..tasks.len() {
            if i == j {
                continue;
            }
            let (mine, theirs) = (&tasks[i], &tasks[j]);
            if mine.repo_path != theirs.repo_path
                || mine.merged_into.is_some()
                || theirs.merged_into.is_some()
            {
                continue;
            }
            let claims = theirs.claims.clone().unwrap_or_default();
            let mut files: Vec<String> = mine
                .changed_files
                .iter()
                .filter(|f| claims.iter().any(|c| claim_covers(c, f)))
                .cloned()
                .collect();
            if files.is_empty() {
                continue;
            }
            files.sort();
            result[i].push(TaskOverlapDto {
                task_id: theirs.id.clone(),
                title: theirs.title.clone(),
                files,
            });
        }
    }
    for (task, conflicts) in tasks.iter_mut().zip(result) {
        task.claim_conflicts = Some(conflicts);
    }
}

fn agent_name(agent: TaskAgent) -> &'static str {
    match agent {
        TaskAgent::ClaudeCode => "Claude Code",
        TaskAgent::Codex => "Codex",
    }
}

const MAX_CLAIMS: usize = 50;

/// Validate and normalize claimed paths: repository-relative, no `..`, folders
/// keep their trailing `/`.
fn normalize_claims(claims: &[String]) -> Result<Vec<String>> {
    if claims.len() > MAX_CLAIMS {
        return Err(Error::Invalid(format!("at most {MAX_CLAIMS} claims")));
    }
    let mut out = Vec::new();
    for claim in claims {
        let claim = claim.trim().trim_start_matches("./");
        if claim.is_empty() {
            continue;
        }
        let is_dir = claim.ends_with('/');
        let path = Path::new(claim.trim_end_matches('/'));
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)) || c.as_os_str() == ".git")
            || claim.len() > 300
        {
            return Err(Error::Invalid(format!(
                "claim must be a repository-relative path: {claim}"
            )));
        }
        out.push(if is_dir {
            format!("{}/", path.display())
        } else {
            path.display().to_string()
        });
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Whether `file` falls under `claim` (exact file, or anything under a folder).
fn claim_covers(claim: &str, file: &str) -> bool {
    match claim.strip_suffix('/') {
        Some(dir) => file.starts_with(&format!("{dir}/")),
        None => file == claim,
    }
}

/// Whether two declared claims name the same path or one contains the other.
fn claims_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    claim_contains(a, b) || claim_contains(b, a)
}

fn claim_contains(outer: &str, inner: &str) -> bool {
    match outer.strip_suffix('/') {
        Some(dir) => inner == dir || inner.starts_with(&format!("{dir}/")),
        None => false,
    }
}

fn overlapping_claim_error(tasks: &[TaskRecord]) -> Option<String> {
    let mut msgs = Vec::new();
    for (i, a) in tasks.iter().enumerate() {
        for b in tasks.iter().skip(i + 1) {
            for ca in &a.claims {
                for cb in &b.claims {
                    if claims_overlap(ca, cb) {
                        let shared = if ca == cb {
                            format!("both claim {ca}")
                        } else {
                            format!("{ca} overlaps {cb}")
                        };
                        msgs.push(format!(
                            "cannot queue \"{}\" and \"{}\": {shared}",
                            a.title, b.title
                        ));
                    }
                }
            }
        }
    }
    msgs.sort();
    msgs.dedup();
    if msgs.is_empty() {
        None
    } else {
        Some(msgs.join("; "))
    }
}

/// What an agent is told about the other agents working in the same repository,
/// prepended to its first prompt (the "shared brain", step 4).
fn coordination_preamble(tasks: &[TaskRecord], task: &TaskRecord) -> String {
    let others: Vec<&TaskRecord> = tasks
        .iter()
        .filter(|t| {
            t.id != task.id
                && t.repo_path == task.repo_path
                && t.merged_into.is_none()
                && matches!(
                    t.state,
                    TaskState::Running | TaskState::Interrupted | TaskState::Finished
                )
        })
        .collect();
    if others.is_empty() && task.claims.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Working alongside other agents (from Lore)\n\
         You are one of several agents working in the same repository, each in its own \
         git worktree. Stay inside your own scope so your work merges cleanly.\n\n",
    );
    if !task.claims.is_empty() {
        out.push_str(&format!("You own: {}\n", task.claims.join(", ")));
    }
    let mut overlaps = Vec::new();
    for other in &others {
        let owns = if other.claims.is_empty() {
            "no declared files".to_string()
        } else {
            other.claims.join(", ")
        };
        out.push_str(&format!(
            "- \"{}\" ({}, {}) owns: {owns}\n",
            other.title,
            agent_name(other.agent),
            state_word(other.state),
        ));
        for mine in &task.claims {
            for theirs in &other.claims {
                if claims_overlap(mine, theirs) {
                    overlaps.push(format!(
                        "Your claim {mine} overlaps \"{}\"'s claim {theirs}.",
                        other.title
                    ));
                }
            }
        }
    }
    if !overlaps.is_empty() {
        overlaps.sort();
        overlaps.dedup();
        out.push('\n');
        out.push_str(&overlaps.join(" "));
        out.push_str(" Lore will refuse to merge both tasks until their claims are distinct.\n");
    }
    let claimed: Vec<&str> = others
        .iter()
        .flat_map(|o| o.claims.iter().map(String::as_str))
        .filter(|theirs| !task.claims.iter().any(|mine| claims_overlap(mine, theirs)))
        .collect();
    if !claimed.is_empty() {
        out.push_str(&format!(
            "\nDo not edit files owned by another agent ({}). If your task needs them, \
             say so in your final message instead of editing them.\n",
            claimed.join(", ")
        ));
    }
    out.push_str("\n## Your task\n");
    out
}

fn state_word(state: TaskState) -> &'static str {
    match state {
        TaskState::Running => "running",
        TaskState::Finished => "finished",
        TaskState::Failed => "failed",
        TaskState::Stopped => "stopped",
        TaskState::Interrupted => "interrupted",
    }
}

/// Context for an agent picking up a task it has no session for.
fn handoff_brief(task: &TaskRecord, prompt: &str, history: &[DecisionDto]) -> String {
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
    let owns = if task.claims.is_empty() {
        "nothing declared; stay within the task's scope".to_string()
    } else {
        task.claims.join(", ")
    };
    let history_lines: Vec<String> = history
        .iter()
        .take(15)
        .map(|d| {
            format!(
                "- {} \"{}\": {}",
                d.kind,
                d.task_title,
                d.detail.chars().take(200).collect::<String>()
            )
        })
        .collect();
    format!(
        "You are continuing a task another agent session started in this git worktree \
         (branch {branch}). Inspect the code yourself before relying on this summary.\n\n\
         ## Original task: {title}\n{original}\n\n\
         ## Files this task owns\n{owns}\n\n\
         ## Commits so far\n{commits}\n\n\
         ## Files changed since the task began\n{changed}\n\n\
         ## Uncommitted paths\n{uncommitted}\n\n\
         ## Recent notes from the previous agent\n{recent}\n\n\
         ## What other agents did in this repository\n{history}\n\n\
         ## What to do now\n{prompt}\n",
        history = if history_lines.is_empty() {
            "(nothing recorded)".to_string()
        } else {
            history_lines.join("\n")
        },
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
/// Tool targets that include the worktree path are shown relative to it.
pub fn activity(snapshot: &TaskSnapshot, max_events: usize) -> Vec<ActivityDto> {
    let worktree = &snapshot.0.worktree_path;
    agent::activity(&snapshot.0.log_path, max_events)
        .into_iter()
        .map(|mut e| {
            e.text = strip_worktree_prefix(&e.text, worktree);
            e
        })
        .collect()
}

fn strip_worktree_prefix(text: &str, worktree: &Path) -> String {
    let wt = worktree.to_string_lossy();
    if wt.is_empty() {
        return text.to_string();
    }
    let slash = if wt.ends_with('/') {
        wt.to_string()
    } else {
        format!("{wt}/")
    };
    if text.contains(&slash) {
        text.replace(&slash, "")
    } else if text.contains(wt.as_ref()) {
        text.replace(wt.as_ref(), "")
    } else {
        text.to_string()
    }
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

    #[test]
    fn claims_overlap_files_and_folders() {
        assert!(claims_overlap("calc.py", "calc.py"));
        assert!(claims_overlap("src/", "src/auth.rs"));
        assert!(claims_overlap("src/auth.rs", "src/"));
        assert!(claims_overlap("src/", "src/auth/"));
        assert!(!claims_overlap("calc.py", "greet.py"));
        assert!(!claims_overlap("src/", "lib.rs"));
    }
}
