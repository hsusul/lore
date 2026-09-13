//! End-to-end task lifecycle against a temp git repo and a fake agent script,
//! so tests never run a real agent or touch real `~/.claude` / `~/.codex`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use lore_ipc::{CreateTaskRequest, TaskAgent, TaskState};
use lore_orchestrator::{AgentPrograms, Orchestrator};

fn sh(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn repo(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    fs::create_dir_all(&repo).unwrap();
    sh(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("README.md"), "hello\n").unwrap();
    sh(&repo, &["add", "README.md"]);
    sh(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

/// A fake agent: records its args, edits a file, commits, prints stream-json.
fn fake_agent(tmp: &Path, body: &str) -> PathBuf {
    let path = tmp.join("fake-agent.sh");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn programs(agent: &Path) -> AgentPrograms {
    AgentPrograms {
        claude_code: agent.to_path_buf(),
        codex: agent.to_path_buf(),
    }
}

fn request(repo: &Path, title: &str) -> CreateTaskRequest {
    CreateTaskRequest {
        repo_path: repo.display().to_string(),
        title: title.into(),
        prompt: "do the thing".into(),
        agent: TaskAgent::ClaudeCode,
        permission: None,
        claims: None,
        auto_handoff: Some(false),
    }
}

fn wait_for(orch: &Orchestrator, id: &str, state: TaskState) -> lore_ipc::TaskDto {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let task = orch.task(id).unwrap();
        if task.state == state {
            return task;
        }
        assert!(Instant::now() < deadline, "task stuck in {:?}", task.state);
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn two_tasks_run_in_isolated_worktrees_and_discard_cleans_up() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    // A dirty primary checkout must not block or leak into worktrees.
    fs::write(repo.join("dirty.txt"), "uncommitted\n").unwrap();

    let agent = fake_agent(
        tmp.path(),
        r#"echo "$PWD" > .agent-cwd
echo work > "work-$$.txt"
git -c user.name=a -c user.email=a@a -c commit.gpgsign=false add -A >/dev/null
git -c user.name=a -c user.email=a@a -c commit.gpgsign=false commit -qm agent >/dev/null
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"committed the work"}]}}'
echo '{"type":"result","result":"All done"}'"#,
    );
    let root = tmp.path().join("orch");
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();

    let a = lore_orchestrator::describe(&orch.create_task(&request(&repo, "Task A")).unwrap());
    let b = lore_orchestrator::describe(&orch.create_task(&request(&repo, "Task A")).unwrap());
    assert_ne!(a.worktree_path, b.worktree_path);
    assert_eq!(a.branch, "lore/task-a");
    assert!(
        b.branch.starts_with("lore/task-a-"),
        "duplicate slug gets a unique branch: {}",
        b.branch
    );

    for id in [&a.id, &b.id] {
        let done = wait_for(&orch, id, TaskState::Finished);
        assert_eq!(done.exit_code, Some(0));
        assert_eq!(done.commits_ahead, 1);
        assert_eq!(done.last_activity.as_deref(), Some("All done"));
        assert!(Path::new(&done.worktree_path).starts_with(root.join("worktrees")));
        let cwd = fs::read_to_string(Path::new(&done.worktree_path).join(".agent-cwd")).unwrap();
        assert_eq!(
            fs::canonicalize(cwd.trim()).unwrap(),
            fs::canonicalize(&done.worktree_path).unwrap()
        );
        assert!(!Path::new(&done.worktree_path).join("dirty.txt").exists());
    }

    // Primary checkout untouched: still on main, dirty file intact, no agent files.
    assert_eq!(
        fs::read_to_string(repo.join("dirty.txt")).unwrap(),
        "uncommitted\n"
    );
    assert!(!repo.join(".agent-cwd").exists());

    orch.discard_task(&a.id).unwrap();
    assert!(!Path::new(&a.worktree_path).exists());
    let branches = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["branch", "--list", "lore/*"])
        .output()
        .unwrap();
    let branches = String::from_utf8_lossy(&branches.stdout);
    assert!(!branches.contains("lore/task-a\n") && branches.contains(&b.branch));
    assert_eq!(orch.list_tasks().unwrap().len(), 1);
}

/// Running and not a zombie (a forgotten `Child` is never reaped in tests).
fn pid_alive(pid: &str) -> bool {
    let out = Command::new("/bin/ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    !stat.trim().is_empty() && !stat.contains('Z')
}

fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn stop_kills_agent_and_everything_it_started() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    // The agent starts a helper (like a test runner) and waits on it.
    let agent = fake_agent(
        tmp.path(),
        "sleep 1000 &\necho $! > \"$PWD/../helper-$$.pid\"\necho started\nwait",
    );
    let root = tmp.path().join("orch");
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();
    let t = orch.create_task(&request(&repo, "long")).unwrap();
    let helper_file = || {
        fs::read_dir(root.join("worktrees"))
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("helper-"))
            .map(|e| e.path())
    };
    // The pid file can exist before the shell has written to it.
    let read_pid = || {
        helper_file()
            .and_then(|f| fs::read_to_string(f).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    assert!(
        wait_until(|| read_pid().is_some()),
        "agent never started its helper"
    );
    let helper_pid = read_pid().unwrap();
    assert!(pid_alive(&helper_pid));

    let stopped = orch.stop_task(t.id()).unwrap();
    assert_eq!(
        lore_orchestrator::describe(&stopped).state,
        TaskState::Stopped
    );
    assert!(
        wait_until(|| !pid_alive(&helper_pid)),
        "helper process survived Stop"
    );
    orch.discard_task(t.id()).unwrap();
}

#[test]
fn restart_marks_running_tasks_interrupted_and_stops_leftover_agents() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    let agent = fake_agent(tmp.path(), "sleep 1000 &\nwait");
    let root = tmp.path().join("orch");

    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();
    let stopped = orch.create_task(&request(&repo, "stopped")).unwrap();
    orch.stop_task(stopped.id()).unwrap();
    let left = orch.create_task(&request(&repo, "left running")).unwrap();
    let left_dto = orch.task(left.id()).unwrap();
    assert_eq!(left_dto.state, TaskState::Running);
    drop(orch); // the app crashed: no shutdown, children never reaped or killed

    let reopened = Orchestrator::open(&root, programs(&agent)).unwrap();
    assert!(reopened.load_warning().is_none());
    assert_eq!(
        reopened.task(stopped.id()).unwrap().state,
        TaskState::Stopped
    );
    assert_eq!(
        reopened.task(left.id()).unwrap().state,
        TaskState::Interrupted
    );
    let store: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("tasks.json")).unwrap()).unwrap();
    let pid = store
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == left.id())
        .unwrap()["pid"]
        .as_u64()
        .unwrap();
    assert!(
        wait_until(|| !pid_alive(&pid.to_string())),
        "leftover agent was not stopped on reopen"
    );
    reopened.discard_task(left.id()).unwrap();
    reopened.discard_task(stopped.id()).unwrap();
}

#[test]
fn corrupt_store_is_moved_aside_instead_of_failing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("orch");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("tasks.json"), "{not json").unwrap();
    let agent = fake_agent(tmp.path(), "true");
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();
    assert!(orch.load_warning().unwrap().contains("could not be read"));
    assert!(orch.list_tasks().unwrap().is_empty());
    assert!(fs::read_dir(&root).unwrap().flatten().any(|e| e
        .file_name()
        .to_string_lossy()
        .starts_with("tasks.json.corrupt-")));
}

#[test]
fn tampered_records_cannot_delete_outside_lore() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    let agent = fake_agent(tmp.path(), "true");
    let root = tmp.path().join("orch");
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();
    let t = orch.create_task(&request(&repo, "victim")).unwrap();
    let id = t.id().to_string();
    drop(orch);

    let store_path = root.join("tasks.json");
    let original = fs::read_to_string(&store_path).unwrap();
    let mut store: serde_json::Value = serde_json::from_str(&original).unwrap();

    // Branch outside lore/ (e.g. main) is refused.
    store[0]["branch"] = "main".into();
    fs::write(&store_path, store.to_string()).unwrap();
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();
    assert!(orch.discard_task(&id).is_err());
    drop(orch);

    // Worktree path escaping the owned directory is refused, and the target survives.
    let outside = tmp.path().join("precious");
    fs::create_dir_all(&outside).unwrap();
    let mut store: serde_json::Value = serde_json::from_str(&original).unwrap();
    store[0]["worktree_path"] = outside.display().to_string().into();
    fs::write(&store_path, store.to_string()).unwrap();
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();
    assert!(orch.discard_task(&id).is_err());
    assert!(outside.exists());
    drop(orch);

    // A symlink inside worktrees/ pointing outside is not followed.
    fs::write(&store_path, &original).unwrap();
    let wt = root.join("worktrees").join(&id);
    Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["worktree", "remove", "--force"])
        .arg(&wt)
        .status()
        .unwrap();
    std::os::unix::fs::symlink(&outside, &wt).unwrap();
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();
    assert!(orch.discard_task(&id).is_err());
    assert!(outside.exists());
    fs::remove_file(&wt).unwrap();

    // Once the path is gone, discard still cleans up the record and branch.
    orch.discard_task(&id).unwrap();
    assert!(orch.list_tasks().unwrap().is_empty());
}

#[test]
fn rejects_bad_input_and_non_repos() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = fake_agent(tmp.path(), "true");
    let orch = Orchestrator::open(tmp.path().join("orch"), programs(&agent)).unwrap();

    let not_repo = tmp.path().join("plain");
    fs::create_dir_all(&not_repo).unwrap();
    assert!(orch.create_task(&request(&not_repo, "x")).is_err());
    assert!(orch
        .create_task(&request(Path::new("relative/path"), "x"))
        .is_err());

    let repo = repo(tmp.path());
    assert!(orch.create_task(&request(&repo, "  ")).is_err());
    let mut bad = request(&repo, "ok");
    bad.prompt = " ".into();
    assert!(orch.create_task(&bad).is_err());
    assert!(orch.list_tasks().unwrap().is_empty());
}

#[test]
fn missing_agent_binary_fails_the_task_instead_of_erroring() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    let orch = Orchestrator::open(
        tmp.path().join("orch"),
        programs(&tmp.path().join("no-such-agent")),
    )
    .unwrap();
    let t = lore_orchestrator::describe(&orch.create_task(&request(&repo, "x")).unwrap());
    assert_eq!(t.state, TaskState::Failed);
    assert!(t.last_activity.unwrap().contains("could not start"));
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_env_repo(tmp: &Path) -> PathBuf {
    let repo = repo(tmp);
    // Identity for commits/merges made by Lore in tests (worktrees share config).
    sh(&repo, &["config", "user.name", "t"]);
    sh(&repo, &["config", "user.email", "t@t"]);
    sh(&repo, &["config", "commit.gpgsign", "false"]);
    repo
}

/// Records each invocation's args, prints a Claude-style init with a session id,
/// and appends a line to the file named by the last argument's first word.
fn recording_agent(tmp: &Path) -> PathBuf {
    fake_agent(
        tmp,
        r#"n=$(ls "$PWD"/../../calls-* 2>/dev/null | wc -l | tr -d ' ')
printf '%s\n' "$@" > "$PWD/../../calls-$n.txt"
echo '{"type":"system","subtype":"init","session_id":"0f8fad5b-d9cb-469f-a165-70867728950e"}'
echo "edit $n" >> shared.txt
echo '{"type":"result","is_error":false,"result":"ok"}'"#,
    )
}

fn call_args(root: &Path, n: usize) -> Vec<String> {
    fs::read_to_string(root.join(format!("calls-{n}.txt")))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn continue_resumes_claude_and_hands_off_to_codex_with_a_brief() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    let agent = recording_agent(tmp.path());
    let root = tmp.path().join("orch");
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();

    let t = orch.create_task(&request(&repo, "shared work")).unwrap();
    let id = t.id().to_string();
    wait_for(&orch, &id, TaskState::Finished);

    // Same agent: resumes its own session with just the new prompt.
    let cont = lore_ipc::ContinueTaskRequest {
        id: id.clone(),
        prompt: "also add docs".into(),
        agent: None,
    };
    orch.continue_task(&cont).unwrap();
    let done = wait_for(&orch, &id, TaskState::Finished);
    assert_eq!(done.runs, Some(2));
    let a1 = call_args(&root, 1);
    assert!(
        a1.iter()
            .any(|a| a == "--resume=0f8fad5b-d9cb-469f-a165-70867728950e"),
        "resumed: {a1:?}"
    );
    assert_eq!(a1.last().unwrap(), "also add docs");

    // Running tasks cannot be continued.
    // Handoff to Codex: no resume, prompt is a brief that carries context.
    let handoff = lore_ipc::ContinueTaskRequest {
        id: id.clone(),
        prompt: "finish the tests".into(),
        agent: Some(TaskAgent::Codex),
    };
    orch.commit_task(&id, "wip").unwrap();
    orch.continue_task(&handoff).unwrap();
    let done = wait_for(&orch, &id, TaskState::Finished);
    assert_eq!(done.agent, TaskAgent::Codex);
    let a2 = call_args(&root, 2);
    assert!(!a2.iter().any(|a| a.starts_with("--resume")));
    // The brief is one multi-line argument; the recorder writes one arg per line.
    let dashdash = a2.iter().position(|a| a == "--").unwrap();
    let brief = a2[dashdash + 1..].join("\n");
    assert!(brief.contains("## Original task: shared work"), "{brief}");
    assert!(brief.contains("wip"), "brief lists commits: {brief}");
    assert!(brief.contains("finish the tests"));
    assert!(done.last_activity.is_some());
}

#[test]
fn commit_and_merge_into_primary_checkout() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    let agent = fake_agent(tmp.path(), "echo feature > feature.txt");
    let orch = Orchestrator::open(tmp.path().join("orch"), programs(&agent)).unwrap();
    let id = orch
        .create_task(&request(&repo, "feature"))
        .unwrap()
        .id()
        .to_string();
    let done = wait_for(&orch, &id, TaskState::Finished);
    assert_eq!(done.uncommitted_count, Some(1));

    // Uncommitted work blocks merging.
    assert!(orch
        .merge_task(&id)
        .unwrap_err()
        .to_string()
        .contains("commit them first"));
    orch.commit_task(&id, "").unwrap();
    assert_eq!(orch.task(&id).unwrap().commits_ahead, 1);
    assert!(orch.commit_task(&id, "again").is_err(), "nothing to commit");

    // A dirty primary checkout blocks merging and is left untouched.
    fs::write(repo.join("README.md"), "local edit\n").unwrap();
    assert!(orch
        .merge_task(&id)
        .unwrap_err()
        .to_string()
        .contains("uncommitted"));
    sh(&repo, &["checkout", "--", "README.md"]);

    let result = orch.merge_task(&id).unwrap();
    assert!(result.merged, "{}", result.message);
    assert_eq!(result.into_branch, "main");
    assert_eq!(
        fs::read_to_string(repo.join("feature.txt")).unwrap(),
        "feature\n"
    );
    assert_eq!(orch.task(&id).unwrap().merged_into.as_deref(), Some("main"));
    assert!(orch
        .continue_task(&lore_ipc::ContinueTaskRequest {
            id: id.clone(),
            prompt: "more".into(),
            agent: None
        })
        .is_err());
}

#[test]
fn conflicting_merge_is_aborted_and_overlaps_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    let agent = fake_agent(tmp.path(), "echo \"$PWD\" > README.md");
    let orch = Orchestrator::open(tmp.path().join("orch"), programs(&agent)).unwrap();
    let a = orch
        .create_task(&request(&repo, "a"))
        .unwrap()
        .id()
        .to_string();
    let b = orch
        .create_task(&request(&repo, "b"))
        .unwrap()
        .id()
        .to_string();
    wait_for(&orch, &a, TaskState::Finished);
    wait_for(&orch, &b, TaskState::Finished);

    let mut all = orch.list_tasks().unwrap();
    lore_orchestrator::annotate_overlaps(&mut all);
    let for_a = all
        .iter()
        .find(|t| t.id == a)
        .unwrap()
        .overlaps
        .clone()
        .unwrap();
    assert_eq!(for_a.len(), 1);
    assert_eq!(for_a[0].task_id, b);
    assert_eq!(for_a[0].files, ["README.md"]);

    orch.commit_task(&a, "a").unwrap();
    orch.commit_task(&b, "b").unwrap();
    assert!(orch.merge_task(&a).unwrap().merged);
    let head = git_out(&repo, &["rev-parse", "HEAD"]);
    let conflict = orch.merge_task(&b).unwrap();
    assert!(!conflict.merged);
    assert_eq!(conflict.conflicts, ["README.md"]);
    // Aborted: same HEAD, clean tree, no merge in progress.
    assert_eq!(git_out(&repo, &["rev-parse", "HEAD"]), head);
    assert_eq!(git_out(&repo, &["status", "--porcelain"]), "");
    assert!(!repo.join(".git/MERGE_HEAD").exists());

    // Merged tasks no longer count as overlapping.
    let mut all = orch.list_tasks().unwrap();
    lore_orchestrator::annotate_overlaps(&mut all);
    assert!(all.iter().all(|t| t.overlaps.as_ref().unwrap().is_empty()));
}

#[test]
fn merge_refuses_a_worktree_off_its_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    let agent = fake_agent(tmp.path(), "echo x > x.txt");
    let orch = Orchestrator::open(tmp.path().join("orch"), programs(&agent)).unwrap();
    let id = orch
        .create_task(&request(&repo, "detach"))
        .unwrap()
        .id()
        .to_string();
    let done = wait_for(&orch, &id, TaskState::Finished);
    orch.commit_task(&id, "x").unwrap();
    // The agent detaches HEAD and commits somewhere the branch ref does not see.
    sh(
        Path::new(&done.worktree_path),
        &["checkout", "-q", "--detach"],
    );
    let head = git_out(&repo, &["rev-parse", "HEAD"]);
    let err = orch.merge_task(&id).unwrap_err().to_string();
    assert!(err.contains("no longer on"), "{err}");
    assert_eq!(git_out(&repo, &["rev-parse", "HEAD"]), head);
    assert!(orch.task(&id).unwrap().merged_into.is_none());
}

#[test]
fn a_second_orchestrator_on_the_same_root_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = fake_agent(tmp.path(), "true");
    let root = tmp.path().join("orch");
    let first = Orchestrator::open(&root, programs(&agent)).unwrap();
    let err = Orchestrator::open(&root, programs(&agent)).err().unwrap();
    assert!(err.to_string().contains("already running"), "{err}");
    drop(first);
    if let Err(e) = Orchestrator::open(&root, programs(&agent)) {
        let holders = Command::new("lsof")
            .arg(root.join("lock"))
            .output()
            .unwrap();
        panic!(
            "reopen failed: {e}\n{}",
            String::from_utf8_lossy(&holders.stdout)
        );
    }
}

#[test]
fn only_opened_roots_and_task_worktrees_are_browsable() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    let other = tmp.path().join("other");
    fs::create_dir_all(&other).unwrap();
    sh(&other, &["init", "-q"]);
    let agent = fake_agent(tmp.path(), "true");
    let orch = Orchestrator::open(tmp.path().join("orch"), programs(&agent)).unwrap();

    let s = |p: &Path| p.display().to_string();
    assert!(orch.browsable_root(&s(&repo)).is_err(), "not opened yet");
    orch.open_workspace(&s(&repo.join("."))).unwrap();
    assert!(orch.browsable_root(&s(&repo)).is_ok());
    assert!(orch.browsable_root(&s(&other)).is_err(), "unrelated repo");

    let t = orch.create_task(&request(&repo, "wt")).unwrap();
    assert!(orch.browsable_root(&s(t.worktree_path())).is_ok());
}

#[test]
fn slow_stop_does_not_block_status_and_helpers_block_commit_until_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    // Ignores SIGTERM, so Stop has to wait out the grace period before SIGKILL.
    let agent = fake_agent(
        tmp.path(),
        "trap '' TERM\necho > edit.txt\nwhile true; do sleep 0.1; done",
    );
    let orch =
        std::sync::Arc::new(Orchestrator::open(tmp.path().join("orch"), programs(&agent)).unwrap());
    let id = orch
        .create_task(&request(&repo, "stubborn"))
        .unwrap()
        .id()
        .to_string();
    std::thread::sleep(Duration::from_millis(300));

    let stopper = {
        let orch = orch.clone();
        let id = id.clone();
        std::thread::spawn(move || orch.stop_task(&id).unwrap())
    };
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    let state = orch
        .snapshot(&id)
        .map(|s| lore_orchestrator::describe(&s).state)
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "status blocked behind Stop"
    );
    assert_eq!(state, TaskState::Stopped);
    stopper.join().unwrap();

    // A finished agent whose helper ignores TERM: commit waits for it to be killed.
    let agent2 = fake_agent(
        tmp.path(),
        "(trap '' TERM; while true; do sleep 0.1; done) &\necho x > y.txt",
    );
    let orch2 = Orchestrator::open(tmp.path().join("orch2"), programs(&agent2)).unwrap();
    let id2 = orch2
        .create_task(&request(&repo, "helper"))
        .unwrap()
        .id()
        .to_string();
    wait_for(&orch2, &id2, TaskState::Finished);
    let err = orch2.commit_task(&id2, "c").unwrap_err().to_string();
    assert!(err.contains("still exiting"), "{err}");
    assert!(
        wait_until(|| {
            let _ = orch2.snapshot(&id2);
            orch2.commit_task(&id2, "c").is_ok()
        }),
        "helper was never killed"
    );
}

#[test]
fn continue_rolls_back_when_the_store_cannot_be_saved() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    let agent = fake_agent(tmp.path(), "sleep 1000 &\necho x > f.txt\nwait");
    let root = tmp.path().join("orch");
    let orch = Orchestrator::open(&root, programs(&fake_agent(tmp.path(), "true"))).unwrap();
    let id = orch
        .create_task(&request(&repo, "r"))
        .unwrap()
        .id()
        .to_string();
    wait_for(&orch, &id, TaskState::Finished);
    orch.set_programs(programs(&agent));

    fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();
    let result = orch.continue_task(&lore_ipc::ContinueTaskRequest {
        id: id.clone(),
        prompt: "more".into(),
        agent: None,
    });
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    let t = orch.task(&id).unwrap();
    assert_eq!(t.state, TaskState::Finished, "state reverted");
    assert_eq!(t.runs, Some(1));
}

#[test]
fn changed_file_totals_and_bounded_diffs() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    let agent = fake_agent(
        tmp.path(),
        "i=0; while [ $i -lt 1100 ]; do echo $i > f$i.txt; i=$((i+1)); done\nhead -c 3000000 /dev/zero | tr '\\0' 'a' > big.txt",
    );
    let orch = Orchestrator::open(tmp.path().join("orch"), programs(&agent)).unwrap();
    let id = orch
        .create_task(&request(&repo, "many"))
        .unwrap()
        .id()
        .to_string();
    let t = wait_for(&orch, &id, TaskState::Finished);
    assert_eq!(t.changed_files.len(), 1000);
    assert_eq!(t.changed_files_total, Some(1101));
    let diff = lore_orchestrator::diff(&orch.snapshot(&id).unwrap()).unwrap();
    assert!(diff.truncated);
    assert!(diff.text.len() <= 2 * 1024 * 1024);
}

#[test]
fn claims_are_shared_with_agents_and_guard_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    let agent = recording_agent(tmp.path());
    let root = tmp.path().join("orch");
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();

    let mut owner = request(&repo, "auth work");
    owner.claims = Some(vec!["src/auth/".into(), "README.md".into()]);
    let owner_id = orch.create_task(&owner).unwrap().id().to_string();
    wait_for(&orch, &owner_id, TaskState::Finished);

    // A second task is told what the first one owns.
    let other_id = orch
        .create_task(&request(&repo, "other work"))
        .unwrap()
        .id()
        .to_string();
    wait_for(&orch, &other_id, TaskState::Finished);
    let prompt = call_args(&root, 1).join("\n");
    assert!(
        prompt.contains("Working alongside other agents"),
        "{prompt}"
    );
    assert!(prompt.contains("\"auth work\""), "{prompt}");
    assert!(prompt.contains("README.md, src/auth/"), "{prompt}");
    assert!(prompt.contains("## Your task"), "{prompt}");

    // The second task edited a claimed file: commit is refused, then forced.
    let wt = orch
        .snapshot(&other_id)
        .unwrap()
        .worktree_path()
        .to_path_buf();
    fs::write(wt.join("README.md"), "trespass\n").unwrap();
    let err = orch.commit_task(&other_id, "x").unwrap_err().to_string();
    assert!(err.contains("another agent owns"), "{err}");
    assert!(err.contains("README.md"), "{err}");
    orch.commit_task_forced(&other_id, "x", true).unwrap();

    let mut all = orch.list_tasks().unwrap();
    lore_orchestrator::annotate_overlaps(&mut all);
    let other = all.iter().find(|t| t.id == other_id).unwrap();
    let conflicts = other.claim_conflicts.clone().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].task_id, owner_id);
    assert_eq!(conflicts[0].files, ["README.md"]);
    assert_eq!(
        all.iter().find(|t| t.id == owner_id).unwrap().claims,
        Some(vec!["README.md".to_string(), "src/auth/".to_string()])
    );

    // Bad claims are refused.
    let mut bad = request(&repo, "bad");
    bad.claims = Some(vec!["../escape".into()]);
    assert!(orch.create_task(&bad).is_err());
}

#[test]
fn decisions_are_recorded_and_reach_the_next_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    let agent = recording_agent(tmp.path());
    let root = tmp.path().join("orch");
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();

    let first = orch
        .create_task(&request(&repo, "first task"))
        .unwrap()
        .id()
        .to_string();
    wait_for(&orch, &first, TaskState::Finished);
    orch.commit_task(&first, "first commit").unwrap();
    assert!(orch.merge_task(&first).unwrap().merged);

    let kinds: Vec<String> = orch
        .decisions(None, 50)
        .into_iter()
        .map(|d| d.kind)
        .collect();
    assert!(kinds.contains(&"created".to_string()));
    assert!(kinds.contains(&"committed".to_string()));
    assert!(kinds.contains(&"merged".to_string()));
    assert_eq!(orch.decisions(Some("/nowhere"), 50).len(), 0);

    // A handoff brief carries that history to the next agent.
    let second = orch
        .create_task(&request(&repo, "second task"))
        .unwrap()
        .id()
        .to_string();
    wait_for(&orch, &second, TaskState::Finished);
    orch.continue_task(&lore_ipc::ContinueTaskRequest {
        id: second.clone(),
        prompt: "keep going".into(),
        agent: Some(TaskAgent::Codex),
    })
    .unwrap();
    wait_for(&orch, &second, TaskState::Finished);
    let brief = call_args(&root, 2).join("\n");
    assert!(
        brief.contains("What other agents did in this repository"),
        "{brief}"
    );
    assert!(brief.contains("first task"), "{brief}");
}

#[test]
fn a_usage_limit_hands_off_to_the_other_agent_once() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = commit_env_repo(tmp.path());
    // Mimics Codex 0.154.0 output when out of quota, then succeeds on the retry.
    let agent = fake_agent(
        tmp.path(),
        r#"n=$(ls "$PWD"/../../calls-* 2>/dev/null | wc -l | tr -d ' ')
printf '%s\n' "$@" > "$PWD/../../calls-$n.txt"
if [ "$n" = "0" ]; then
  echo '{"type":"error","message":"You'"'"'ve hit your usage limit. Try again at 4:30 PM."}'
  exit 1
fi
echo '{"type":"result","is_error":false,"result":"picked it up"}'"#,
    );
    let root = tmp.path().join("orch");
    let orch = Orchestrator::open(&root, programs(&agent)).unwrap();
    let mut req = request(&repo, "limited");
    req.auto_handoff = Some(true);
    let id = orch.create_task(&req).unwrap().id().to_string();
    let failed = wait_for(&orch, &id, TaskState::Failed);
    assert!(failed.attention.unwrap().contains("Usage limit"));

    assert_eq!(orch.run_auto_handoffs(), vec![id.clone()]);
    let after = wait_for(&orch, &id, TaskState::Finished);
    assert_eq!(after.agent, TaskAgent::Codex, "handed to the other agent");
    assert_eq!(after.runs, Some(2));
    // Only once per run, and the handoff is in the shared log.
    assert!(orch.run_auto_handoffs().is_empty());
    assert!(orch
        .decisions(None, 20)
        .iter()
        .any(|d| d.kind == "auto_handoff"));
}
