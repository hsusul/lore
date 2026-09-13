//! # lore-orchestrator
//!
//! Runs coding agents in parallel, each in its own Lore-owned git worktree
//! (ADR-0007, `docs/product/ORCHESTRATOR_PLAN.md` step 1).
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
//!
//! Locking: `snapshots`/`snapshot` are cheap. `create_task`, `continue_task`,
//! `commit_task`, `merge_task`, `stop_task`, and `discard_task` run git or wait
//! on processes while holding the orchestrator. Git-derived status comes from
//! [`describe`], which callers run on a [`TaskSnapshot`] outside any lock.

pub mod agent;
mod git;
mod process;
pub mod workspace;

use std::collections::HashMap;
use std::fs::{self, File};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use lore_ipc::{
    ActivityDto, ActivityKind, ContinueTaskRequest, CreateTaskRequest, MergeResultDto, TaskAgent,
    TaskDiffDto, TaskDto, TaskOverlapDto, TaskPermission, TaskState,
};
use serde::{Deserialize, Serialize};
use std::io::Write;

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
const BRANCH_PREFIX: &str = "lore/";
const MAX_TITLE: usize = 200;
const MAX_PROMPT: usize = 100_000;
const MAX_CHANGED_FILES: usize = 200;

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

/// Owns task records, their worktrees, and the agent processes it started.
pub struct Orchestrator {
    root: PathBuf,
    programs: AgentPrograms,
    tasks: Vec<TaskRecord>,
    children: HashMap<String, Child>,
    load_warning: Option<String>,
}

impl Orchestrator {
    /// Open (or create) the orchestrator rooted at `root`.
    ///
    /// Never fails because of a damaged store: an unreadable `tasks.json` is
    /// moved aside and reported via [`Orchestrator::load_warning`]. Tasks that
    /// were running when the previous process exited become `Interrupted`, and
    /// any of their agents still alive are stopped.
    pub fn open(root: impl Into<PathBuf>, programs: AgentPrograms) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("worktrees"))?;
        fs::create_dir_all(root.join("logs"))?;
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
            programs,
            tasks,
            children: HashMap::new(),
            load_warning,
        };
        if changed {
            orchestrator.save()?;
        }
        Ok(orchestrator)
    }

    /// Why previously saved tasks could not be loaded, if that happened.
    pub fn load_warning(&self) -> Option<&str> {
        self.load_warning.as_deref()
    }

    /// Replace the programs used for tasks launched from now on.
    pub fn set_programs(&mut self, programs: AgentPrograms) {
        self.programs = programs;
    }

    /// Create a worktree for the request and launch its agent there.
    pub fn create_task(&mut self, req: &CreateTaskRequest) -> Result<TaskSnapshot> {
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

        match self.spawn(&record, &record.prompt, None) {
            Ok(child) => {
                record.pid = Some(child.id());
                self.children.insert(id.clone(), child);
            }
            Err(e) => {
                record.state = TaskState::Failed;
                let _ = fs::write(
                    &record.log_path,
                    format!("Lore could not start the agent: {e}\n"),
                );
            }
        }
        self.tasks.push(record);
        if let Err(e) = self.save() {
            // Without a record nothing could ever clean this up, so roll back.
            let _ = self.discard_task(&id);
            return Err(e);
        }
        self.snapshot(&id)
    }

    /// Start one agent run for `task`, appending its output to the task log.
    fn spawn(&self, task: &TaskRecord, prompt: &str, resume: Option<&str>) -> Result<Child> {
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
            &self.programs,
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

    /// Send more work to a task that is not running, in the same worktree.
    ///
    /// Keeping the agent resumes its own session when it supports that (Claude);
    /// otherwise, including every switch of agent (a handoff), the new run gets a
    /// brief built from the task's commits, changes, and recent activity.
    pub fn continue_task(&mut self, req: &ContinueTaskRequest) -> Result<TaskSnapshot> {
        self.reap()?;
        let index = self.index(&req.id)?;
        let prompt = req.prompt.trim();
        if prompt.is_empty() || prompt.len() > MAX_PROMPT {
            return Err(Error::Invalid(
                "prompt must be non-empty and under 100 KB".into(),
            ));
        }
        let task = &self.tasks[index];
        if task.state == TaskState::Running {
            return Err(Error::Invalid("the task is still running".into()));
        }
        if task.merged_into.is_some() {
            return Err(Error::Invalid("the task was already merged".into()));
        }
        if !task.worktree_path.is_dir() {
            return Err(Error::Invalid(
                "the task's worktree no longer exists".into(),
            ));
        }
        let new_agent = req.agent.unwrap_or(task.agent);
        let session = (new_agent == task.agent && new_agent == TaskAgent::ClaudeCode)
            .then(|| agent::claude_session_id(&task.log_path))
            .flatten();
        let full_prompt = match session {
            Some(_) => prompt.to_string(),
            None => handoff_brief(task, prompt),
        };

        let mut record = task.clone();
        record.agent = new_agent;
        record.runs += 1;
        record.exit_code = None;
        match self.spawn(&record, &full_prompt, session.as_deref()) {
            Ok(child) => {
                record.pid = Some(child.id());
                record.state = TaskState::Running;
                self.children.insert(record.id.clone(), child);
            }
            Err(e) => {
                record.state = TaskState::Failed;
                record.pid = None;
                let _ = fs::OpenOptions::new()
                    .append(true)
                    .open(&record.log_path)
                    .and_then(|mut f| writeln!(f, "Lore could not start the agent: {e}"));
            }
        }
        self.tasks[index] = record;
        self.save()?;
        self.snapshot(&req.id)
    }

    /// Commit every uncommitted change in the task's worktree.
    pub fn commit_task(&mut self, id: &str, message: &str) -> Result<TaskSnapshot> {
        self.reap()?;
        let task = &self.tasks[self.index(id)?];
        if task.state == TaskState::Running {
            return Err(Error::Invalid("stop the agent before committing".into()));
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
        self.snapshot(id)
    }

    /// Merge the task's branch into the branch checked out in its repository.
    ///
    /// Refuses while the agent runs, while the worktree has uncommitted changes,
    /// or while the primary checkout has uncommitted changes. A conflicting merge
    /// is aborted, leaving the checkout exactly as it was.
    pub fn merge_task(&mut self, id: &str) -> Result<MergeResultDto> {
        self.reap()?;
        let index = self.index(id)?;
        let task = self.tasks[index].clone();
        if task.state == TaskState::Running {
            return Err(Error::Invalid("stop the agent before merging".into()));
        }
        if let Some(into) = &task.merged_into {
            return Err(Error::Invalid(format!(
                "the task was already merged into {into}"
            )));
        }
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
                self.tasks[index].merged_into = Some(into.clone());
                self.save()?;
                Ok(MergeResultDto {
                    merged: true,
                    message: format!("Merged {} into {into}.", task.branch),
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
    pub fn snapshots(&mut self) -> Result<Vec<TaskSnapshot>> {
        self.reap()?;
        let mut out: Vec<TaskSnapshot> = self.tasks.iter().cloned().map(TaskSnapshot).collect();
        out.sort_by_key(|t| std::cmp::Reverse(t.0.created_at_ms));
        Ok(out)
    }

    /// Refresh process states and snapshot one task.
    pub fn snapshot(&mut self, id: &str) -> Result<TaskSnapshot> {
        self.reap()?;
        self.tasks
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .map(TaskSnapshot)
            .ok_or(Error::NotFound)
    }

    /// Convenience for callers that do not care about lock scope (tests, CLI).
    pub fn list_tasks(&mut self) -> Result<Vec<TaskDto>> {
        Ok(self.snapshots()?.iter().map(describe).collect())
    }

    /// Convenience: one task with git-derived status.
    pub fn task(&mut self, id: &str) -> Result<TaskDto> {
        self.snapshot(id).map(|s| describe(&s))
    }

    /// Stop a running agent and everything it started. No-op otherwise.
    pub fn stop_task(&mut self, id: &str) -> Result<TaskSnapshot> {
        self.reap()?;
        let index = self.index(id)?;
        if self.tasks[index].state == TaskState::Running {
            self.kill_task_processes(index);
            self.tasks[index].state = TaskState::Stopped;
            self.save()?;
        }
        self.snapshot(id)
    }

    /// Stop the agent, remove the worktree and its branch, and forget the task.
    /// Uncommitted and unmerged work in that worktree is lost.
    pub fn discard_task(&mut self, id: &str) -> Result<()> {
        let index = self.index(id)?;
        let task = self.tasks[index].clone();
        let owned = self.root.join("worktrees");

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

        self.kill_task_processes(index);

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
        self.tasks.remove(index);
        self.save()
    }

    /// Stop every running agent (called when the app quits). No git work.
    pub fn shutdown(&mut self) {
        let running: Vec<usize> = (0..self.tasks.len())
            .filter(|&i| self.tasks[i].state == TaskState::Running)
            .collect();
        for index in running {
            self.kill_task_processes(index);
            self.tasks[index].state = TaskState::Stopped;
        }
        let _ = self.save();
    }

    fn kill_task_processes(&mut self, index: usize) {
        let task = &self.tasks[index];
        let mut child = self.children.remove(&task.id);
        if let Some(pid) = task.pid {
            // Only signal a group we started in this process, or one whose
            // leader is verifiably still our agent (guards against pid reuse).
            if child.is_some()
                || process::runs_program(pid, program_for(&self.programs, task.agent))
            {
                process::stop_group(pid, child.as_mut());
            }
        } else if let Some(c) = child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn index(&self, id: &str) -> Result<usize> {
        self.tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or(Error::NotFound)
    }

    /// Record the exit of any finished agents, and clean up what they left
    /// running in their process group.
    fn reap(&mut self) -> Result<()> {
        let mut done = Vec::new();
        for (id, child) in &mut self.children {
            if let Ok(Some(status)) = child.try_wait() {
                done.push((id.clone(), status, child.id()));
            }
        }
        if done.is_empty() {
            return Ok(());
        }
        for (id, status, pid) in done {
            self.children.remove(&id);
            process::signal_group(pid, "TERM");
            if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                task.exit_code = status.code().map(i64::from);
                task.state = if status.success() {
                    TaskState::Finished
                } else {
                    TaskState::Failed
                };
            }
        }
        self.save()
    }

    fn save(&self) -> Result<()> {
        let tmp = self.root.join(format!("{STORE_FILE}.tmp"));
        {
            let file = File::create(&tmp)?;
            serde_json::to_writer_pretty(&file, &self.tasks)?;
            file.sync_all()?;
        }
        fs::rename(tmp, self.root.join(STORE_FILE))?;
        Ok(())
    }
}

/// Full status for a task, including git-derived fields. Runs git subprocesses,
/// so call it without holding the orchestrator.
pub fn describe(snapshot: &TaskSnapshot) -> TaskDto {
    let t = &snapshot.0;
    let exists = t.worktree_path.is_dir();
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
        changed_files: if exists {
            git::changed_files(&t.worktree_path, &t.base_commit, MAX_CHANGED_FILES)
        } else {
            Vec::new()
        },
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
    let changed = git::changed_files(wt, &task.base_commit, 100);
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
