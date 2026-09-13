//! Parallel agent tasks (ADR-0007, `ORCHESTRATOR_PLAN.md` step 1).
//!
//! Thin wrappers over `lore-orchestrator`, run on Tauri's blocking pool. The
//! orchestrator locks only around bookkeeping; git, process waits, and status
//! derivation (`describe`) run without it.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use lore_ipc::{
    ActivityDto, ContinueTaskRequest, CreateTaskRequest, DecisionDto, DirEntryDto, FileContentDto,
    MergeResultDto, TaskDiffDto, TaskDto,
};
use lore_orchestrator::{describe, AgentPrograms, TaskSnapshot};
use tauri::async_runtime::spawn_blocking;
use tauri::{AppHandle, Manager, State};

use crate::state::AppState;

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Err("invalid task id".to_string())
    } else {
        Ok(())
    }
}

/// Run `f` against the orchestrator on the blocking pool. The orchestrator
/// locks internally only around bookkeeping, so slow git and process work in
/// one command does not stall others.
async fn with_orchestrator<T, F>(app: AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&lore_orchestrator::Orchestrator) -> Result<T, String> + Send + 'static,
{
    spawn_blocking(move || f(&app.state::<AppState>().orchestrator))
        .await
        .map_err(|e| e.to_string())?
}

async fn describe_all(snapshots: Vec<TaskSnapshot>) -> Result<Vec<TaskDto>, String> {
    spawn_blocking(move || snapshots.iter().map(describe).collect())
        .await
        .map_err(|e| e.to_string())
}

async fn describe_one(snapshot: TaskSnapshot) -> Result<TaskDto, String> {
    spawn_blocking(move || describe(&snapshot))
        .await
        .map_err(|e| e.to_string())
}

/// List orchestrated tasks with live status, newest first.
#[tauri::command]
pub async fn list_tasks(app: AppHandle) -> Result<Vec<TaskDto>, String> {
    let snapshots = with_orchestrator(app, |o| o.snapshots().map_err(|e| e.to_string())).await?;
    let mut tasks = describe_all(snapshots).await?;
    lore_orchestrator::annotate_overlaps(&mut tasks);
    Ok(tasks)
}

/// Send more work to a stopped/finished task, optionally handing off to the
/// other agent.
#[tauri::command]
pub async fn continue_task(
    app: AppHandle,
    request: ContinueTaskRequest,
) -> Result<TaskDto, String> {
    check_id(&request.id)?;
    let snapshot = with_orchestrator(app, move |o| {
        o.continue_task(&request).map_err(|e| e.to_string())
    })
    .await?;
    describe_one(snapshot).await
}

/// Commit all uncommitted changes in a task's worktree. `force` commits even
/// when the task changed files another task owns.
#[tauri::command]
pub async fn commit_task(
    app: AppHandle,
    id: String,
    message: String,
    force: Option<bool>,
) -> Result<TaskDto, String> {
    check_id(&id)?;
    let snapshot = with_orchestrator(app, move |o| {
        o.commit_task_forced(&id, &message, force.unwrap_or(false))
            .map_err(|e| e.to_string())
    })
    .await?;
    describe_one(snapshot).await
}

/// One task with fresh status, for refreshing after a change event.
#[tauri::command]
pub async fn get_task(app: AppHandle, id: String) -> Result<TaskDto, String> {
    check_id(&id)?;
    let snapshot =
        with_orchestrator(app, move |o| o.snapshot(&id).map_err(|e| e.to_string())).await?;
    describe_one(snapshot).await
}

/// The shared decision log: what agents did across tasks, newest first.
#[tauri::command]
pub async fn list_decisions(
    app: AppHandle,
    repo_path: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<DecisionDto>, String> {
    with_orchestrator(app, move |o| {
        Ok(o.decisions(
            repo_path.as_deref(),
            limit.unwrap_or(100).min(1_000) as usize,
        ))
    })
    .await
}

/// Merge a task's branch into the repository's checked-out branch.
#[tauri::command]
pub async fn merge_task(app: AppHandle, id: String) -> Result<MergeResultDto, String> {
    check_id(&id)?;
    with_orchestrator(app, move |o| o.merge_task(&id).map_err(|e| e.to_string())).await
}

/// Why saved tasks could not be loaded at startup, if that happened.
#[tauri::command]
pub fn task_load_warning(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.orchestrator.load_warning())
}

/// Create a Lore-owned worktree for the request and launch its agent.
#[tauri::command]
pub async fn create_task(app: AppHandle, request: CreateTaskRequest) -> Result<TaskDto, String> {
    let needs_resolve = !app
        .state::<AppState>()
        .agents_resolved
        .load(Ordering::Relaxed);
    let resolved = if needs_resolve {
        Some(
            spawn_blocking(resolve_agent_programs)
                .await
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    if let Some((programs, complete)) = resolved {
        let state = app.state::<AppState>();
        state.orchestrator.set_programs(programs);
        // Only stop looking once both agents were found, so installing one
        // later works without restarting Lore.
        if complete {
            state.agents_resolved.store(true, Ordering::Relaxed);
        }
    }
    let snapshot = with_orchestrator(app, move |o| {
        o.create_task(&request).map_err(|e| e.to_string())
    })
    .await?;
    describe_one(snapshot).await
}

/// Stop a running task's agent and everything it started.
#[tauri::command]
pub async fn stop_task(app: AppHandle, id: String) -> Result<TaskDto, String> {
    check_id(&id)?;
    let snapshot =
        with_orchestrator(app, move |o| o.stop_task(&id).map_err(|e| e.to_string())).await?;
    describe_one(snapshot).await
}

/// Stop the agent and delete the task's worktree and branch.
#[tauri::command]
pub async fn discard_task(app: AppHandle, id: String) -> Result<(), String> {
    check_id(&id)?;
    with_orchestrator(app, move |o| o.discard_task(&id).map_err(|e| e.to_string())).await
}

/// Reveal a task's worktree in Finder.
#[tauri::command]
pub async fn open_task_worktree(app: AppHandle, id: String) -> Result<(), String> {
    check_id(&id)?;
    let snapshot =
        with_orchestrator(app, move |o| o.snapshot(&id).map_err(|e| e.to_string())).await?;
    let path = snapshot.worktree_path().to_path_buf();
    if !path.is_dir() {
        return Err("the worktree no longer exists".into());
    }
    let status = Command::new("/usr/bin/open")
        .arg(&path)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("could not open worktree".into())
    }
}

/// List one directory of a workspace (a repository or task worktree root).
#[tauri::command]
pub async fn list_workspace_dir(
    root: String,
    rel_path: String,
) -> Result<Vec<DirEntryDto>, String> {
    spawn_blocking(move || lore_orchestrator::workspace::list_dir(&root, &rel_path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Read a workspace file for display (read-only, capped at 1 MB).
#[tauri::command]
pub async fn read_workspace_file(
    app: AppHandle,
    root: String,
    rel_path: String,
) -> Result<FileContentDto, String> {
    with_orchestrator(app, move |o| {
        let root = o.browsable_root(&root).map_err(|e| e.to_string())?;
        lore_orchestrator::workspace::read_file(&root.to_string_lossy(), &rel_path)
            .map_err(|e| e.to_string())
    })
    .await
}

/// Resolve a user-chosen folder to its repository top-level.
#[tauri::command]
pub async fn open_workspace(path: String) -> Result<String, String> {
    spawn_blocking(move || {
        let top =
            lore_orchestrator::workspace::repository_root(&path).map_err(|e| e.to_string())?;
        Ok(top.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Unified diff of a task's worktree against its base commit.
#[tauri::command]
pub async fn task_diff(app: AppHandle, id: String) -> Result<TaskDiffDto, String> {
    check_id(&id)?;
    let snapshot =
        with_orchestrator(app, move |o| o.snapshot(&id).map_err(|e| e.to_string())).await?;
    spawn_blocking(move || lore_orchestrator::diff(&snapshot))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// The task agent's recent activity timeline, oldest first.
#[tauri::command]
pub async fn task_activity(app: AppHandle, id: String) -> Result<Vec<ActivityDto>, String> {
    check_id(&id)?;
    let snapshot =
        with_orchestrator(app, move |o| o.snapshot(&id).map_err(|e| e.to_string())).await?;
    spawn_blocking(move || lore_orchestrator::activity(&snapshot, 500))
        .await
        .map_err(|e| e.to_string())
}

/// Apps launched from Finder get a minimal `PATH`, so `claude` / `codex`
/// installed via npm, Homebrew, or their installers are usually not on it.
/// Ask the user's login shell (bounded by a timeout), then check common install
/// locations. Returns the programs and whether both agents were found.
fn resolve_agent_programs() -> (AgentPrograms, bool) {
    let defaults = AgentPrograms::default();
    let claude = resolve("claude");
    let codex = resolve("codex");
    let complete = claude.is_some() && codex.is_some();
    (
        AgentPrograms {
            claude_code: claude.unwrap_or(defaults.claude_code),
            codex: codex.unwrap_or(defaults.codex),
        },
        complete,
    )
}

fn resolve(name: &str) -> Option<PathBuf> {
    from_login_shell(name).or_else(|| {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        [
            home.join(".local/bin").join(name),
            home.join(".claude/local").join(name),
            home.join(".npm-global/bin").join(name),
            PathBuf::from("/opt/homebrew/bin").join(name),
            PathBuf::from("/usr/local/bin").join(name),
        ]
        .into_iter()
        .find(|p| p.is_file())
    })
}

fn from_login_shell(name: &str) -> Option<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let dir = std::env::temp_dir().join(format!("lore-which-{}-{name}", std::process::id()));
    let out_path = dir.with_extension("out");
    let out = std::fs::File::create(&out_path).ok()?;
    let mut child = Command::new(&shell)
        .args(["-lic", &format!("command -v {name}")])
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + RESOLVE_TIMEOUT;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => break false,
        }
    };
    if !finished {
        let _ = child.kill();
        let _ = child.wait();
    }
    let text = std::fs::read_to_string(&out_path).unwrap_or_default();
    let _ = std::fs::remove_file(&out_path);
    text.lines()
        .map(str::trim)
        .rfind(|l| l.starts_with('/'))
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}
