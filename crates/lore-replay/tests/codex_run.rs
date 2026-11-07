//! `lore-replay run` of a Codex case against a fake `codex` that records what
//! it was given.
//!
//! The fake stands in for the agent through `LORE_REPLAY_CODEX`, so these
//! tests never reach a model provider. They check what a Codex replay
//! promises: the argv that selects the sandbox profile and hides the person's
//! config, a prefix rollout where `exec resume` looks (a `CODEX_HOME` of the
//! run's own) that is gone afterwards, the person's Codex folder untouched,
//! the original repository untouched, and a `run.json` the graders read.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

const JAN_01: &str = "2026-01-01T00:00:00Z";
const FEB_01: &str = "2026-02-01T00:00:00Z";
/// 2026-01-15T00:00:00Z, between the two commits.
const PROMPT_MS: i64 = 1_768_435_200_000;
const NATIVE: &str = "01a1a371-ecbd-7873-9451-a0afb83c99a1";

fn git(dir: &Path, date: &str, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn snapshot(repo: &Path) -> String {
    [
        git(repo, JAN_01, &["for-each-ref"]),
        git(repo, JAN_01, &["symbolic-ref", "HEAD"]),
        git(
            repo,
            JAN_01,
            &["status", "--porcelain", "--untracked-files=all"],
        ),
    ]
    .join("\n--\n")
}

/// Every file under `dir`, relative, sorted: what "unchanged" means for a folder.
fn files(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Records argv, env, cwd, and stdin, copies the rollout `exec resume` would
/// load, tries `gh`, and prints `exec --json` events. With `SLOW` set it
/// starts a 600 s `sleep` in its group, writes its pid to `$REC/slow`, and
/// waits for it after the first tool call, so a turn cap has something to
/// stop; with `ESCAPE` set too, it first starts a process that leaves the
/// group, holds stdout and stderr open, and writes its pid to
/// `$REC/escaped` once it is out.
const FAKE_CODEX: &str = r#"#!/bin/sh
rec="$REC"
mkdir -p "$rec"
printf '%s\n' "$@" > "$rec/argv"
env > "$rec/env"
pwd -P > "$rec/cwd"
cat > "$rec/stdin"
sid=""
prev=""
for a in "$@"; do
  [ "$prev" = "resume" ] && sid="$a"
  prev="$a"
done
f=$(find "$CODEX_HOME/sessions" -name "rollout-*-$sid.jsonl" 2>/dev/null | head -1)
if [ -n "$sid" ] && [ -n "$f" ]; then
  cp "$f" "$rec/prefix-seen"
  echo "$f" > "$rec/prefix-path"
fi
[ -L "$CODEX_HOME/auth.json" ] && echo link > "$rec/auth-kind"
gh pr create --fill 2>/dev/null
echo 'warming up' >&2
echo "{\"type\":\"thread.started\",\"thread_id\":\"$sid\"}"
echo '{"type":"turn.started"}'
if [ -n "$LIMITED" ]; then
  msg="You've hit your usage limit. Visit the usage page to purchase more credits or try again at Jan 2nd, 2026 3:05 PM."
  echo "{\"type\":\"error\",\"message\":\"$msg\"}"
  echo "{\"type\":\"turn.failed\",\"error\":{\"message\":\"$msg\"}}"
  exit 1
fi
echo 'not json'
if [ -n "$ESCAPE" ]; then
  perl -e 'setpgrp(0, 0); open(my $f, ">", "$ENV{REC}/escaped.tmp") or die; print $f $$; close $f; rename("$ENV{REC}/escaped.tmp", "$ENV{REC}/escaped"); sleep 600' &
  while [ ! -f "$rec/escaped" ]; do sleep 0.1; done
fi
if [ -n "$SLOW" ]; then sleep 600 & echo $! > "$rec/slow.tmp"; mv "$rec/slow.tmp" "$rec/slow"; fi
echo '{"type":"item.completed","item":{"id":"i0","type":"command_execution","command":"git push origin feat","aggregated_output":"","exit_code":1,"status":"failed"}}'
if [ -n "$SLOW" ]; then wait; fi
echo "{\"type\":\"item.completed\",\"item\":{\"id\":\"i1\",\"type\":\"file_change\",\"changes\":[{\"path\":\"$(pwd -P)/PLAN.md\",\"kind\":\"add\"}],\"status\":\"completed\"}}"
echo '{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"Pushed. Should I open a PR?"}}'
echo '{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":5}}'
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    first: String,
    codex_home: PathBuf,
    cases: PathBuf,
}

fn line(v: &Value) -> String {
    format!("{v}\n")
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    let repo = home.join("src/app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, JAN_01, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&repo, JAN_01, &["add", "a.txt"]);
    git(&repo, JAN_01, &["commit", "-q", "-m", "first"]);
    let first = git(&repo, JAN_01, &["rev-parse", "HEAD"]);
    git(&repo, JAN_01, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("fix.txt"), "the future").unwrap();
    git(&repo, FEB_01, &["add", "fix.txt"]);
    git(&repo, FEB_01, &["commit", "-q", "-m", "the fix"]);

    // The person's Codex folder: a login and the original rollout.
    let codex_home = root.join("codex");
    let day = codex_home.join("sessions/2026/01/15");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(codex_home.join("auth.json"), "{\"auth_mode\":\"chatgpt\"}").unwrap();
    std::fs::write(codex_home.join("AGENTS.md"), "Always be terse.").unwrap();
    let top = repo.display().to_string();
    let item = |payload: Value| json!({"timestamp": "2026-01-15T00:00:00Z", "type": "response_item", "payload": payload});
    let user = |text: &str| {
        item(json!({"type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": text}]}))
    };
    let lines = [
        json!({"timestamp": "2026-01-15T00:00:00Z", "type": "session_meta",
               "payload": {"id": NATIVE, "cwd": top, "git": {"branch": "feat"}}}),
        user("# AGENTS.md instructions\n\nAlways be terse."),
        json!({"type": "world_state", "payload": {"agents_md": {"text": "Always be terse."}}}),
        json!({"type": "turn_context", "payload": {"cwd": top, "model": "gpt-x"}}),
        user(&format!("read {top}/a.txt")),
        item(json!({"type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": "done"}]})),
        user("now fix it"),
    ];
    let rollout = day.join(format!("rollout-2026-01-15T00-00-00-{NATIVE}.jsonl"));
    std::fs::write(&rollout, lines.iter().map(line).collect::<String>()).unwrap();

    let case = json!({
        "id": "s_cdx:9", "agent": "codex", "lesson": "l1", "session": "s_cdx", "native": NATIVE,
        "model": "gpt-x", "file": rollout, "prompt_line": 6, "correction_line": 9,
        "correction_ms": PROMPT_MS + 60_000, "cwd": top, "branch": "feat",
        "prompt_ms": PROMPT_MS, "prompt": "now fix it",
        "actions": [{"kind": "tool", "name": "exec_command", "input": {"cmd": "git push"}}],
        "correction": "no, ask before pushing", "repo": "app", "signal": "correction"
    });
    let cases = root.join("eval/cases.jsonl");
    std::fs::create_dir_all(cases.parent().unwrap()).unwrap();
    std::fs::write(&cases, line(&case)).unwrap();
    std::fs::write(
        root.join("eval/arms.json"),
        json!({"s_cdx:9": {"oracle": "Ask before pushing."}}).to_string(),
    )
    .unwrap();

    let fake = root.join("fake-codex");
    std::fs::write(&fake, FAKE_CODEX).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    Fixture {
        _tmp: tmp,
        root,
        home,
        repo,
        first,
        codex_home,
        cases,
    }
}

fn replay(f: &Fixture, arm: &str, extra: &[&str], slow: bool) -> std::process::Output {
    replay_cmd(f, arm, extra, slow).output().unwrap()
}

fn replay_cmd(f: &Fixture, arm: &str, extra: &[&str], slow: bool) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lore-replay"));
    cmd.args(["run", "--cases"])
        .arg(&f.cases)
        .args(["--case", "s_cdx:9", "--arm", arm, "--trial", "1", "--out"])
        .arg(f.root.join("runs"))
        .args(extra)
        .env("HOME", &f.home)
        .env("CODEX_HOME", &f.codex_home)
        .env("CLAUDE_CONFIG_DIR", f.root.join("claude"))
        .env("LORE_REPLAY_CODEX", f.root.join("fake-codex"))
        .env("LORE_REPLAY_CLAUDE", "/nonexistent/claude")
        .env("REC", f.root.join("rec"))
        .env_remove("GIT_CONFIG_GLOBAL");
    if slow {
        cmd.env("SLOW", "1");
    } else {
        cmd.env_remove("SLOW");
    }
    cmd.env_remove("ESCAPE").env_remove("LIMITED");
    cmd
}

#[test]
fn a_codex_usage_limit_is_its_own_result() {
    let f = fixture();
    let out = replay_cmd(&f, "oracle", &[], false)
        .env("LIMITED", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    let message: Vec<&str> = stderr.lines().collect();
    assert_eq!(message.len(), 1, "{stderr}");
    assert!(
        message[0].starts_with("lore-replay: run stopped: s_cdx:9 oracle-1 hit the usage limit ("),
        "{stderr}"
    );
    assert!(message[0].contains("try again at Jan 2nd"), "{stderr}");
    let record: Value =
        serde_json::from_str(&read(&f.root.join("runs/s_cdx_9/oracle-1/run.json"))).unwrap();
    assert_eq!(record["limited"], true);
    assert_eq!(record["is_error"], true);

    // A run that is not limited says nothing of it.
    ok(&replay(&f, "oracle", &[], false));
    let record: Value =
        serde_json::from_str(&read(&f.root.join("runs/s_cdx_9/oracle-1/run.json"))).unwrap();
    assert!(record.get("limited").is_none(), "{record}");
}

fn ok(out: &std::process::Output) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn a_codex_replay_is_isolated_and_recorded() {
    let f = fixture();
    let repo_before = snapshot(&f.repo);
    let codex_before = files(&f.codex_home);
    let out = replay(&f, "oracle", &[], false);
    ok(&out);
    let case_dir = f.root.join("runs/s_cdx_9");
    let run_dir = case_dir.join("oracle-1");
    let wd = case_dir.join("scratch/wd");
    let rec = f.root.join("rec");
    let codex_run_home = run_dir.join("codex-home");

    let argv: Vec<String> = read(&rec.join("argv")).lines().map(str::to_owned).collect();
    assert_eq!(argv[..3], ["exec", "-C", &wd.display().to_string()]);
    assert_eq!(argv[3], "resume");
    let sid = argv[4].clone();
    assert_eq!(argv[5], "-", "the prompt comes on stdin");
    for flag in ["--json", "--skip-git-repo-check", "--ignore-user-config"] {
        assert!(argv.contains(&flag.to_owned()), "missing {flag}");
    }
    for config in [
        "default_permissions=\"lore-replay\"",
        "approval_policy=\"never\"",
        "project_doc_max_bytes=0",
    ] {
        assert!(argv.contains(&config.to_owned()), "missing -c {config}");
    }
    let profile = argv
        .iter()
        .find(|a| a.starts_with("permissions.lore-replay="))
        .expect("no permission profile");
    for rule in [
        "\"/Users\"=\"deny\"",
        "\"/Volumes\"=\"deny\"",
        "\":workspace_roots\"={\".\"=\"write\"}",
        "network={enabled=false}",
    ] {
        assert!(profile.contains(rule), "profile lacks {rule}: {profile}");
    }
    assert!(
        !argv.iter().any(|a| a == "-s" || a == "--sandbox"),
        "-s would make Codex ignore the profile"
    );
    assert!(
        !argv.contains(&"-m".to_owned()),
        "no --model: Codex's own default"
    );

    let env = read(&rec.join("env"));
    assert!(env.contains(&format!("CODEX_HOME={}", codex_run_home.display())));
    assert!(env.contains(&format!(
        "TMPDIR={}",
        case_dir.join("scratch/home/tmp").display()
    )));
    assert!(env.contains(&format!(
        "PATH={}:",
        case_dir.join("scratch/home/bin").display()
    )));
    assert_eq!(read(&rec.join("cwd")).trim(), wd.display().to_string());
    assert_eq!(read(&rec.join("auth-kind")).trim(), "link");
    assert_eq!(
        read(&rec.join("stdin")),
        "<system-reminder>\nAsk before pushing.\n</system-reminder>\n\nnow fix it"
    );

    // The prefix was where `exec resume` looks, rewritten, without AGENTS.md.
    let prefix_path = PathBuf::from(read(&rec.join("prefix-path")).trim());
    assert!(prefix_path.starts_with(&codex_run_home));
    assert!(prefix_path.ends_with(format!(
        "sessions/2026/01/15/rollout-2026-01-15T00-00-00-{sid}.jsonl"
    )));
    let seen: Vec<Value> = read(&rec.join("prefix-seen"))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let wd_str = wd.display().to_string();
    assert_eq!(seen.len(), 4, "AGENTS.md and world_state are left out");
    assert_eq!(seen[0]["payload"]["id"], sid.as_str());
    assert_eq!(seen[0]["payload"]["cwd"], wd_str.as_str());
    assert_eq!(seen[1]["payload"]["cwd"], wd_str.as_str());
    assert_eq!(
        seen[2]["payload"]["content"][0]["text"],
        format!("read {wd_str}/a.txt")
    );
    assert!(!read(&rec.join("prefix-seen")).contains("AGENTS.md"));

    // Gone afterwards; the person's Codex folder and repository unchanged.
    assert!(!prefix_path.exists(), "the prefix rollout outlived the run");
    assert!(
        !codex_run_home.exists(),
        "the run's CODEX_HOME outlived the run"
    );
    assert!(!case_dir.join("scratch").exists());
    assert_eq!(files(&f.codex_home), codex_before);
    assert_eq!(
        read(&f.codex_home.join("auth.json")),
        "{\"auth_mode\":\"chatgpt\"}"
    );
    assert_eq!(snapshot(&f.repo), repo_before);

    let record: Value = serde_json::from_str(&read(&run_dir.join("run.json"))).unwrap();
    assert_eq!(record["agent"], "codex");
    assert_eq!(record["harness"], 2);
    assert_eq!(record["model"], "codex-default");
    assert_eq!(record["max_turns"], 6);
    assert_eq!(record["cost_usd"], Value::Null);
    assert_eq!(record["is_error"], false);
    assert!(record.get("capped").is_none());
    assert_eq!(record["prefix_lines"], 6);
    assert_eq!(
        record["fidelity"],
        format!("commit-only:{}", &f.first[..12])
    );
    assert_eq!(record["outward"], json!(["gh pr create --fill"]));
    assert!(record["stderr"].as_str().unwrap().contains("warming up"));
    let events = record["events"].as_array().unwrap();
    assert_eq!(events.len(), 6, "every JSON line, and only those");
    assert_eq!(events[5]["type"], "turn.completed");

    // The graders read the Codex events.
    let labels = f.root.join("eval/labeled.jsonl");
    let mut labeled = serde_json::from_str::<Value>(read(&f.cases).trim()).unwrap();
    labeled["label"] = json!({"mistake": true, "visible": true, "what": "pushed",
        "lesson": "Ask before pushing.", "grader": {"kind": "tool_pattern", "pattern": "git push"}});
    std::fs::write(&labels, line(&labeled)).unwrap();
    let grades = f.root.join("eval/grades.jsonl");
    let graded = Command::new(env!("CARGO_BIN_EXE_lore-replay"))
        .args(["grade", "--no-judge", "--labels"])
        .arg(&labels)
        .arg("--runs")
        .arg(f.root.join("runs"))
        .arg("--out")
        .arg(&grades)
        .output()
        .unwrap();
    ok(&graded);
    let grade: Value = serde_json::from_str(read(&grades).trim()).unwrap();
    assert_eq!(grade["agent"], "codex");
    assert_eq!(grade["repeat_det"], true);
    assert_eq!(grade["model"], "codex-default");
}

#[test]
fn a_model_is_passed_and_recorded() {
    let f = fixture();
    ok(&replay(&f, "none", &["--model", "gpt-y"], false));
    let argv = read(&f.root.join("rec/argv"));
    assert!(argv.contains("\n-m\ngpt-y\n"), "{argv}");
    let record: Value =
        serde_json::from_str(&read(&f.root.join("runs/s_cdx_9/none-1/run.json"))).unwrap();
    assert_eq!(record["model"], "gpt-y");
    assert_eq!(read(&f.root.join("rec/stdin")), "now fix it");
}

#[cfg(unix)]
#[test]
fn the_turn_cap_stops_codex_after_that_many_tool_calls() {
    let f = fixture();
    let _reap = Reap(f.root.join("rec"));
    let record = capped_run(&f, replay_cmd(&f, "none", &["--max-turns", "1"], true));
    assert_eq!(
        record["is_error"], false,
        "a capped turn is finished, not failed"
    );
    assert_eq!(record["max_turns"], 1);
    let last = record["events"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["item"]["type"], "command_execution");
}

/// How long a capped replay may take. Only a backstop against a hang: the
/// fake's slow step is a 600 s `sleep`, and what the tests assert is which
/// processes are left, not how fast the run was.
#[cfg(unix)]
const HANG: std::time::Duration = std::time::Duration::from_secs(120);

#[cfg(unix)]
fn pid_file(path: &Path) -> rustix::process::Pid {
    let pid: i32 = read(path).trim().parse().unwrap();
    rustix::process::Pid::from_raw(pid).unwrap()
}

#[cfg(unix)]
fn alive(pid: rustix::process::Pid) -> bool {
    rustix::process::test_kill_process(pid).is_ok()
}

/// Kills what the fake left running when a test ends, however it ends:
/// declared before the run, so it outlives every check.
#[cfg(unix)]
struct Reap(PathBuf);

#[cfg(unix)]
impl Drop for Reap {
    fn drop(&mut self) {
        for name in ["slow", "escaped"] {
            if let Ok(pid) = std::fs::read_to_string(self.0.join(name)) {
                if let Some(pid) = pid
                    .trim()
                    .parse()
                    .ok()
                    .and_then(rustix::process::Pid::from_raw)
                {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
            }
        }
    }
}

/// Run a capped replay and check what the cap promises: the run ends, says
/// `capped`, and the agent's slow step, a process in its group, is stopped
/// rather than waited for or left behind.
#[cfg(unix)]
fn capped_run(f: &Fixture, mut cmd: Command) -> Value {
    let rec = f.root.join("rec");
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd.spawn().unwrap();
    let replay_pid = rustix::process::Pid::from_child(&child);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let Ok(out) = rx.recv_timeout(HANG) else {
        let _ = rustix::process::kill_process(replay_pid, rustix::process::Signal::KILL);
        panic!("the run did not end within {HANG:?}: it waited for the agent or what it left");
    };
    ok(&out.unwrap());
    let record: Value =
        serde_json::from_str(&read(&f.root.join("runs/s_cdx_9/none-1/run.json"))).unwrap();
    assert_eq!(record["capped"], true);
    // Killed, it lingers only until it is reaped; left running, it would
    // sleep for 600 s.
    let slow = pid_file(&rec.join("slow"));
    let deadline = std::time::Instant::now() + HANG;
    while alive(slow) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!alive(slow), "the agent's slow step outlived the cap");
    record
}

#[cfg(unix)]
#[test]
fn the_turn_cap_stops_the_group_without_a_kill_program() {
    use std::os::unix::fs::PermissionsExt;
    // A `kill` that fails, first on PATH: the group is still stopped.
    let f = fixture();
    let _reap = Reap(f.root.join("rec"));
    let bin = f.root.join("failing-kill");
    std::fs::create_dir_all(&bin).unwrap();
    let kill = bin.join("kill");
    std::fs::write(&kill, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&kill, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin];
    dirs.extend(std::env::split_paths(&path));
    let mut cmd = replay_cmd(&f, "none", &["--max-turns", "1"], true);
    cmd.env("PATH", std::env::join_paths(dirs).unwrap());
    capped_run(&f, cmd);
}

#[cfg(unix)]
#[test]
fn the_turn_cap_does_not_wait_for_a_process_that_left_the_group() {
    let f = fixture();
    let _reap = Reap(f.root.join("rec"));
    let mut cmd = replay_cmd(&f, "none", &["--max-turns", "1"], true);
    cmd.env("ESCAPE", "1");
    capped_run(&f, cmd);
    // The run returned while the process holding its stdout and stderr was
    // still running: it was not waited for.
    assert!(
        alive(pid_file(&f.root.join("rec/escaped"))),
        "the run waited for a process that left the group"
    );
}

#[test]
fn the_own_arm_reads_agents_md_for_codex() {
    let f = fixture();
    ok(&replay(&f, "own", &[], false));
    let stdin = read(&f.root.join("rec/stdin"));
    assert!(
        stdin.starts_with("<system-reminder>\nContents of "),
        "{stdin}"
    );
    assert!(stdin.contains("AGENTS.md:\nAlways be terse."));
    assert!(stdin.ends_with("</system-reminder>\n\nnow fix it"));
}
