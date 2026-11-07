//! `lore-replay run` against a fake `claude` that records what it was given.
//!
//! The fake stands in for the agent through `LORE_REPLAY_CLAUDE`, so these
//! tests never reach a model provider. They check the isolation a replay
//! promises: the flags and switches that hide the person's setup, the prefix
//! under a symlinked project folder that is gone afterwards, a scratch clone
//! at the commit of the prompt, and an original repository left untouched.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

const JAN_01: &str = "2026-01-01T00:00:00Z";
const JAN_02: &str = "2026-01-02T00:00:00Z";
const FEB_01: &str = "2026-02-01T00:00:00Z";
/// 2026-01-15T00:00:00Z, between the two commits.
const PROMPT_MS: i64 = 1_768_435_200_000;

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
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// Refs, HEAD, and working-tree status: what "unchanged" means for a repo.
fn snapshot(repo: &Path) -> String {
    [
        git(repo, JAN_01, &["for-each-ref"]),
        git(repo, JAN_01, &["symbolic-ref", "HEAD"]),
        git(
            repo,
            JAN_01,
            &["status", "--porcelain", "--untracked-files=all"],
        ),
        git(repo, JAN_01, &["stash", "list"]),
    ]
    .join("\n--\n")
}

const FAKE_CLAUDE: &str = r#"#!/bin/sh
rec="$REC"
mkdir -p "$rec"
if [ -n "$SPAN_LOG" ]; then
  echo start >> "$SPAN_LOG"
  sleep 1
  echo end >> "$SPAN_LOG"
fi
printf '%s\n' "$@" > "$rec/argv"
env > "$rec/env"
pwd -P > "$rec/cwd"
cat > "$rec/stdin"
sid=""
prev=""
for a in "$@"; do
  [ "$prev" = "--resume" ] && sid="$a"
  prev="$a"
done
proj="$CLAUDE_CONFIG_DIR/projects/$(pwd -P | sed 's/[^A-Za-z0-9]/-/g')"
[ -L "$proj" ] && echo symlink > "$rec/project-kind"
[ -n "$sid" ] && cat "$proj/$sid.jsonl" > "$rec/prefix-seen"
gh pr create --fill 2>/dev/null
echo 'warming up' >&2
echo '{"type":"system","subtype":"init"}'
echo 'not json'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"ok"}]}}'
echo '{"type":"result","total_cost_usd":0.12,"is_error":false}'
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    first: String,
    claude_dir: PathBuf,
    cases: PathBuf,
}

fn line(v: &Value) -> String {
    format!("{v}\n")
}

fn fixture(prompt_line: usize) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    let repo = home.join("src/app");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = Pat Q\n\temail = pat@example.com\n",
    )
    .unwrap();
    git(&repo, JAN_01, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&repo, JAN_01, &["add", "a.txt"]);
    git(&repo, JAN_01, &["commit", "-q", "-m", "first"]);
    let first = git(&repo, JAN_01, &["rev-parse", "HEAD"]);
    git(&repo, JAN_02, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("fix.txt"), "the future").unwrap();
    git(&repo, FEB_01, &["add", "fix.txt"]);
    git(&repo, FEB_01, &["commit", "-q", "-m", "the fix"]);
    std::fs::write(repo.join("dirty.txt"), "uncommitted").unwrap();

    let claude_dir = root.join("claude");
    let project = claude_dir.join("projects/-orig");
    std::fs::create_dir_all(&project).unwrap();
    let top = repo.display().to_string();
    let session = project.join("native-1.jsonl");
    let lines = [
        json!({"type": "user", "sessionId": "native-1", "cwd": top,
               "message": {"content": format!("read {top}/a.txt and ~/src/app/a.txt")}}),
        json!({"type": "assistant", "sessionId": "native-1", "cwd": top,
               "message": {"content": [{"type": "text", "text": "done"}]}}),
        json!({"type": "user", "sessionId": "native-1", "cwd": top,
               "message": {"content": "now fix it"}}),
    ];
    std::fs::write(&session, lines.iter().map(line).collect::<String>()).unwrap();

    let case = json!({
        "id": "s_abc:4", "lesson": "l1", "session": "s_abc", "native": "native-1",
        "model": null, "file": session, "prompt_line": prompt_line, "correction_line": 4,
        "correction_ms": PROMPT_MS + 60_000, "cwd": top, "branch": "feat",
        "prompt_ms": PROMPT_MS, "prompt": "now fix it",
        "actions": [{"kind": "tool", "name": "Bash",
                     "input": {"command": format!("cat {top}/a.txt /Users/someone/notes.md")}}],
        "correction": "no, the other way", "repo": "app", "signal": "correction"
    });
    let cases = root.join("eval/cases.jsonl");
    std::fs::create_dir_all(cases.parent().unwrap()).unwrap();
    std::fs::write(&cases, line(&case)).unwrap();
    std::fs::write(
        root.join("eval/arms.json"),
        json!({"s_abc:4": {"oracle": "Do it the other way.", "lore": "x"}}).to_string(),
    )
    .unwrap();

    let fake = root.join("fake-claude");
    std::fs::write(&fake, FAKE_CLAUDE).unwrap();
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
        claude_dir,
        cases,
    }
}

fn command(f: &Fixture, arm: &str, trial: u32, rec: &Path, extra: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lore-replay"));
    cmd.args(["run", "--cases"])
        .arg(&f.cases)
        .args(["--case", "s_abc:4", "--arm", arm, "--trial"])
        .arg(trial.to_string())
        .arg("--out")
        .arg(f.root.join("runs"))
        .args(extra)
        .env("HOME", &f.home)
        .env("CLAUDE_CONFIG_DIR", &f.claude_dir)
        .env("LORE_REPLAY_CLAUDE", f.root.join("fake-claude"))
        .env("REC", rec)
        .env_remove("SPAN_LOG")
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("CLAUDE_CODE_DISABLE_CLAUDE_MDS")
        .env_remove("CLAUDE_CODE_DISABLE_AUTO_MEMORY");
    cmd
}

fn replay(f: &Fixture, arm: &str, extra: &[&str]) -> std::process::Output {
    command(f, arm, 1, &f.root.join("rec"), extra)
        .output()
        .unwrap()
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
fn a_replay_is_isolated_and_recorded() {
    let f = fixture(2);
    let before = snapshot(&f.repo);
    let out = replay(&f, "oracle", &["--keep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run_dir = f.root.join("runs/s_abc_4/oracle-1");
    let scratch = f.root.join("runs/s_abc_4/scratch");
    let wd = scratch.join("wd");
    let rec = f.root.join("rec");

    let argv: Vec<String> = read(&rec.join("argv")).lines().map(str::to_owned).collect();
    let after = |flag: &str| {
        let i = argv
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("no {flag}"));
        argv[i + 1].clone()
    };
    assert_eq!(argv[0], "-p");
    for flag in [
        "--fork-session",
        "--no-session-persistence",
        "--verbose",
        "--strict-mcp-config",
    ] {
        assert!(argv.contains(&flag.to_owned()), "missing {flag}");
    }
    assert_eq!(after("--setting-sources"), "");
    assert_eq!(after("--model"), "sonnet");
    assert_eq!(after("--max-turns"), "6");
    assert_eq!(after("--output-format"), "stream-json");
    assert!(!argv.contains(&"--append-system-prompt".to_owned()));
    let settings: Value = serde_json::from_str(&after("--settings")).unwrap();
    assert_eq!(
        settings["sandbox"]["filesystem"]["denyRead"],
        json!(["/Users", "/Volumes"])
    );
    let sid = after("--resume");

    let env = read(&rec.join("env"));
    for var in [
        "CLAUDE_CODE_DISABLE_CLAUDE_MDS=1",
        "CLAUDE_CODE_DISABLE_AUTO_MEMORY=1",
    ] {
        assert!(env.lines().any(|l| l == var), "env lacks {var}");
    }
    let gitconfig = scratch.join("home/gitconfig");
    assert!(env.contains(&format!("GIT_CONFIG_GLOBAL={}", gitconfig.display())));
    assert!(read(&gitconfig).contains("Pat Q"));
    assert!(env.contains(&format!(
        "XDG_CONFIG_HOME={}",
        scratch.join("home/xdg").display()
    )));
    assert!(env.contains(&format!("PATH={}:", scratch.join("home/bin").display())));
    assert!(env.contains(&format!(
        "LORE_REPLAY_OUTWARD_LOG={}",
        run_dir.join("outward.log").display()
    )));

    assert_eq!(read(&rec.join("cwd")).trim(), wd.display().to_string());
    assert_eq!(
        read(&rec.join("stdin")),
        "<system-reminder>\nDo it the other way.\n</system-reminder>\n\nnow fix it"
    );
    assert_eq!(
        git(&wd, JAN_01, &["rev-parse", "HEAD"]),
        f.first,
        "HEAD at the prompt"
    );
    assert_eq!(
        git(&wd, JAN_01, &["symbolic-ref", "--short", "HEAD"]),
        "feat"
    );
    assert!(
        !wd.join("fix.txt").exists(),
        "a later commit leaked into the clone"
    );
    assert_eq!(
        git(&wd, JAN_01, &["remote"]),
        "",
        "the clone keeps no remote"
    );

    assert_eq!(read(&rec.join("project-kind")).trim(), "symlink");
    let seen: Vec<Value> = read(&rec.join("prefix-seen"))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(seen.len(), 2);
    let wd_str = wd.display().to_string();
    assert!(seen
        .iter()
        .all(|e| e["sessionId"] == sid.as_str() && e["cwd"] == wd_str.as_str()));
    assert_eq!(
        seen[0]["message"]["content"],
        format!("read {wd_str}/a.txt and {wd_str}/a.txt")
    );
    let slug: String = wd_str
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let link = f.claude_dir.join("projects").join(slug);
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "the project symlink outlived the run"
    );
    assert!(run_dir
        .join("claude-project")
        .join(format!("{sid}.jsonl"))
        .is_file());

    let record: Value = serde_json::from_str(&read(&run_dir.join("run.json"))).unwrap();
    let mut keys: Vec<&str> = record
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "agent",
            "arm",
            "case",
            "cost_usd",
            "delivery",
            "events",
            "fidelity",
            "foreign_paths",
            "harness",
            "is_error",
            "max_turns",
            "model",
            "outward",
            "prefix_lines",
            "seconds",
            "stderr",
            "trial",
            "trim"
        ]
    );
    assert_eq!(record["trim"], 0);
    assert_eq!(record["max_turns"], 6);
    assert_eq!(record["case"], "s_abc:4");
    assert_eq!(record["harness"], 2);
    assert_eq!(record["delivery"], "user-message");
    assert_eq!(record["arm"], "oracle");
    assert_eq!(record["trial"], 1);
    assert_eq!(
        record["fidelity"],
        format!("commit-only:{}", &f.first[..12])
    );
    assert_eq!(record["foreign_paths"], json!(["/Users/someone/notes.md"]));
    assert_eq!(record["prefix_lines"], 2);
    assert_eq!(record["cost_usd"], 0.12);
    assert_eq!(record["is_error"], false);
    assert_eq!(record["stderr"], "warming up\n");
    assert_eq!(record["outward"], json!(["gh pr create --fill"]));
    assert_eq!(record["events"].as_array().unwrap().len(), 3);

    assert_eq!(snapshot(&f.repo), before, "the original repository changed");
}

#[test]
fn an_empty_prefix_starts_a_fresh_session() {
    let f = fixture(0);
    let out = replay(&f, "none", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let argv = read(&f.root.join("rec/argv"));
    assert!(!argv
        .lines()
        .any(|l| l == "--resume" || l == "--fork-session"));
    assert!(!argv.lines().any(|l| l == "--append-system-prompt"));
}

#[test]
fn an_arm_without_its_text_is_refused() {
    let f = fixture(2);
    let out = replay(&f, "file", &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no `file` text"));
    assert!(
        !f.root.join("rec/argv").exists(),
        "the agent must not start"
    );
}

#[test]
fn a_run_deletes_its_scratch_and_keeps_the_record() {
    let f = fixture(2);
    let out = replay(&f, "oracle", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run_dir = f.root.join("runs/s_abc_4/oracle-1");
    for scratch in [
        f.root.join("runs/s_abc_4/scratch"),
        run_dir.join("claude-project"),
    ] {
        assert!(!scratch.exists(), "{} outlived the run", scratch.display());
    }
    assert!(run_dir.join("run.json").is_file());
}

/// Every arm and trial of a case sends the same argv from the same folder,
/// so they share a prompt-cache prefix; only the final user message differs.
#[test]
fn every_run_of_a_case_shares_the_cache_prefix() {
    let f = fixture(2);
    std::fs::write(
        f.root.join("eval/arms.json"),
        json!({"s_abc:4": {"file": "FILE", "lore": "LORE", "oracle": "ORACLE"}}).to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(&f.claude_dir).unwrap();
    std::fs::write(f.claude_dir.join("CLAUDE.md"), "OWN").unwrap();
    let mut seen = Vec::new();
    for (arm, trial) in [
        ("none", 1),
        ("none", 2),
        ("own", 1),
        ("file", 1),
        ("lore", 1),
        ("oracle", 1),
        ("oracle", 2),
    ] {
        let rec = f.root.join(format!("rec-{arm}-{trial}"));
        ok(&command(&f, arm, trial, &rec, &[]).output().unwrap());
        seen.push((
            arm,
            std::fs::read(rec.join("argv")).unwrap(),
            read(&rec.join("cwd")),
            read(&rec.join("stdin")),
        ));
    }
    let (_, argv0, cwd0, _) = &seen[0];
    assert!(!String::from_utf8_lossy(argv0).contains("--append-system-prompt"));
    assert!(String::from_utf8_lossy(argv0).contains("--resume"));
    for (arm, argv, cwd, stdin) in &seen {
        assert_eq!(argv, argv0, "argv differs for {arm}");
        assert_eq!(cwd, cwd0, "cwd differs for {arm}");
        let want = match *arm {
            "none" => "now fix it".to_owned(),
            "own" => format!(
                "<system-reminder>\nContents of {}:\nOWN\n</system-reminder>\n\nnow fix it",
                f.claude_dir.join("CLAUDE.md").display()
            ),
            other => format!(
                "<system-reminder>\n{}\n</system-reminder>\n\nnow fix it",
                other.to_uppercase()
            ),
        };
        assert_eq!(stdin, &want, "stdin for {arm}");
    }
}

/// Two runs of one case share its scratch, so the second waits for the first.
#[test]
fn runs_of_one_case_never_overlap() {
    let f = fixture(2);
    let spans = f.root.join("spans");
    let children: Vec<_> = [1, 2]
        .into_iter()
        .map(|trial| {
            command(&f, "none", trial, &f.root.join(format!("rec-{trial}")), &[])
                .env("SPAN_LOG", &spans)
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        ok(&child.wait_with_output().unwrap());
    }
    assert_eq!(read(&spans), "start\nend\nstart\nend\n");
}
