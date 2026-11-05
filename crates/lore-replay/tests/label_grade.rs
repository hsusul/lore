//! `lore-replay label` and `grade` against a fake `claude`.
//!
//! The fake answers by the kind of prompt on its stdin, and records each
//! call's argv and environment, so these tests never reach a model provider.
//! They check the judge's isolation flags, that the prompt goes on stdin,
//! resume, and that a failed judge call stops the run without writing a
//! non-answer.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
n=$(ls "$REC" 2>/dev/null | grep -c '^argv-')
n=$((n + 1))
printf '%s\n' "$@" > "$REC/argv-$n"
env > "$REC/env-$n"
cat > "$REC/stdin-$n"
if [ -n "$FAIL" ]; then
  echo '{"type":"result","is_error":true,"result":"rate limited"}'
  exit 1
fi
if grep -q 'You label correction points' "$REC/stdin-$n"; then
  echo '{"type":"result","is_error":false,"result":"[{\"id\":\"c:1\",\"mistake\":true,\"what\":\"opened a PR\",\"lesson\":\"Do not open PRs.\",\"grader\":{\"kind\":\"tool_pattern\",\"pattern\":\"gh pr create\"},\"visible\":true},{\"id\":\"c:2\",\"mistake\":true,\"what\":\"w\",\"lesson\":\"l\",\"grader\":{\"kind\":\"text_pattern\",\"pattern\":\"(bad\"},\"visible\":true}]"}'
elif grep -q 'You grade one replayed' "$REC/stdin-$n"; then
  echo '{"type":"result","is_error":false,"result":"YES. It opened a PR again."}'
else
  echo '{"type":"result","is_error":false,"result":"NO. Nothing about PRs."}'
fi
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(root.join("rec")).unwrap();
        std::fs::create_dir_all(root.join("claude")).unwrap();
        let fake = root.join("fake-claude");
        std::fs::write(&fake, FAKE_CLAUDE).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cases = ["c:1", "c:2", "c:3"]
            .iter()
            .map(|id| {
                json!({"id": id, "prompt": "ship it", "correction": "no PRs",
                       "actions": [{"kind": "tool", "name": "Bash", "input": {"command": "gh pr create"}}]})
                .to_string()
                    + "\n"
            })
            .collect::<String>();
        std::fs::write(root.join("cases.jsonl"), cases).unwrap();
        Self { _tmp: tmp, root }
    }

    fn cmd(&self, args: &[&str], fail: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_lore-replay"));
        cmd.args(args)
            .current_dir(&self.root)
            .env("LORE_REPLAY_CLAUDE", self.root.join("fake-claude"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("REC", self.root.join("rec"))
            .env_remove("CLAUDE_CODE_DISABLE_CLAUDE_MDS")
            .env_remove("CLAUDE_CODE_DISABLE_AUTO_MEMORY");
        if fail {
            cmd.env("FAIL", "1");
        }
        cmd.output().unwrap()
    }

    fn calls(&self) -> usize {
        std::fs::read_dir(self.root.join("rec"))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("argv-")
            })
            .count()
    }

    fn write_run(&self, case: &str, arm: &str, trial: u32, command: &str, is_error: bool) {
        let dir = self
            .root
            .join("runs")
            .join(case.replace(':', "_"))
            .join(format!("{arm}-{trial}"));
        std::fs::create_dir_all(&dir).unwrap();
        let mut run = json!({
            "case": case, "arm": arm, "trial": trial, "model": "sonnet",
            "fidelity": "commit-only:0123456789ab", "foreign_paths": [], "prefix_lines": 3,
            "seconds": 1.0, "cost_usd": 0.5, "is_error": is_error, "stderr": "", "outward": [],
            "events": [{"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": "Bash", "input": {"command": command}},
                {"type": "text", "text": "done"}]}}],
        });
        // The none runs are harness 2; the rest are older records, without it.
        if arm == "none" {
            run["harness"] = json!(2);
        }
        std::fs::write(dir.join("run.json"), run.to_string()).unwrap();
    }
}

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn label_asks_an_isolated_judge_and_resumes() {
    let f = Fixture::new();
    let args = [
        "label",
        "--cases",
        "cases.jsonl",
        "--out",
        "labeled.jsonl",
        "--batch",
        "8",
    ];
    let summary = ok(&f.cmd(&args, false));
    assert!(summary.contains("2 labeled now (2 mistakes), 1 without a usable label"));
    assert_eq!(f.calls(), 1);

    let argv = std::fs::read_to_string(f.root.join("rec/argv-1")).unwrap();
    let argv: Vec<&str> = argv.lines().collect();
    assert_eq!(
        argv,
        [
            "-p",
            "--model",
            "sonnet",
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--tools",
            "",
            "--output-format",
            "json"
        ]
    );
    let env = std::fs::read_to_string(f.root.join("rec/env-1")).unwrap();
    assert!(env.contains("CLAUDE_CODE_DISABLE_CLAUDE_MDS=1"));
    assert!(env.contains("CLAUDE_CODE_DISABLE_AUTO_MEMORY=1"));
    let stdin = std::fs::read_to_string(f.root.join("rec/stdin-1")).unwrap();
    assert!(stdin.contains("\"id\": \"c:3\""));

    let labeled = lines(&f.root.join("labeled.jsonl"));
    assert_eq!(labeled.len(), 2);
    assert_eq!(labeled[0]["labeled_by"], "judge: claude sonnet");
    assert_eq!(labeled[0]["prompt"], "ship it");
    assert_eq!(labeled[0]["label"]["grader"]["kind"], "tool_pattern");
    // An invalid regex from the model is a judge grader, its pattern the hint.
    assert_eq!(labeled[1]["label"]["grader"]["kind"], "judge");
    assert!(labeled[1]["label"]["grader"]["rubric"]
        .as_str()
        .unwrap()
        .contains("(bad"));

    // Resume: only the case left without a label is asked about again.
    let again = ok(&f.cmd(&args, false));
    assert!(again.contains("2 already labeled"), "{again}");
    assert_eq!(f.calls(), 2);
    let stdin = std::fs::read_to_string(f.root.join("rec/stdin-2")).unwrap();
    assert!(stdin.contains("c:3") && !stdin.contains("\"c:1\""));
}

#[test]
fn grade_writes_both_verdicts_skips_errors_and_resumes() {
    let f = Fixture::new();
    ok(&f.cmd(
        &["label", "--cases", "cases.jsonl", "--out", "labeled.jsonl"],
        false,
    ));
    let label_calls = f.calls();
    f.write_run("c:1", "none", 1, "gh pr create --fill", false);
    f.write_run("c:1", "lore", 1, "ls", false);
    f.write_run("c:1", "lore", 2, "ls", true);
    std::fs::write(
        f.root.join("arms.json"),
        json!({"c:1": {"lore": "Never open PRs.", "file": ""}}).to_string(),
    )
    .unwrap();
    std::fs::write(f.root.join("claude/CLAUDE.md"), "Be brief.").unwrap();
    let args = [
        "grade",
        "--labels",
        "labeled.jsonl",
        "--runs",
        "runs",
        "--out",
        "grades.jsonl",
        "--arms",
        "arms.json",
        "--shown-out",
        "shown.jsonl",
    ];

    // A failed judge stops the run and records nothing for that run.
    let failed = f.cmd(&args, true);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("judge gave no answer"));
    assert!(lines(&f.root.join("grades.jsonl")).is_empty());

    let summary = ok(&f.cmd(&args, false));
    assert!(
        summary.contains("2 graded now; skipped 1 errored"),
        "{summary}"
    );
    let grades = lines(&f.root.join("grades.jsonl"));
    assert_eq!(grades.len(), 2);
    let none = grades.iter().find(|g| g["arm"] == "none").unwrap();
    assert_eq!(none["repeat_det"], true);
    assert_eq!(none["judge"], "YES");
    assert_eq!(none["grader"], "tool_pattern");
    assert_eq!(none["graded_by"], "deterministic+judge: claude sonnet");
    assert_eq!(none["fidelity"], "commit-only:0123456789ab");
    assert_eq!(none["harness"], 2);
    let lore = grades.iter().find(|g| g["arm"] == "lore").unwrap();
    assert_eq!(lore["harness"], 1, "a record without harness is harness 1");
    assert_eq!(lore["repeat_det"], false);

    // One shown verdict per arm; the empty file arm is NO without asking.
    let shown = lines(&f.root.join("shown.jsonl"));
    assert_eq!(shown.len(), 3);
    let by_arm = |arm: &str| shown.iter().find(|s| s["arm"] == arm).unwrap().clone();
    assert_eq!(by_arm("file")["shown"], "NO");
    assert_eq!(by_arm("file")["reason"], "the arm's context is empty");
    assert_eq!(by_arm("lore")["shown"], "NO");
    // 1 failed + 2 runs + lore + own (today's CLAUDE.md, from the config dir).
    assert_eq!(f.calls() - label_calls, 5);
    let own_stdin =
        std::fs::read_to_string(f.root.join(format!("rec/stdin-{}", f.calls()))).unwrap();
    assert!(own_stdin.contains("Be brief."));

    let again = ok(&f.cmd(&args, false));
    assert!(again.contains("2 already graded, 0 graded now"), "{again}");
    assert!(again.contains("0 judged now, 3 already"), "{again}");
    assert_eq!(f.calls() - label_calls, 5);
}

#[test]
fn grade_without_the_judge_never_starts_claude() {
    let f = Fixture::new();
    ok(&f.cmd(
        &["label", "--cases", "cases.jsonl", "--out", "labeled.jsonl"],
        false,
    ));
    let before = f.calls();
    f.write_run("c:2", "none", 1, "ls", false);
    f.write_run("c:1", "none", 1, "ls", false);
    ok(&f.cmd(
        &[
            "grade",
            "--labels",
            "labeled.jsonl",
            "--runs",
            "runs",
            "--out",
            "det.jsonl",
            "--no-judge",
        ],
        true,
    ));
    assert_eq!(f.calls(), before);
    let grades = lines(&f.root.join("det.jsonl"));
    let c2 = grades.iter().find(|g| g["case"] == "c:2").unwrap();
    assert_eq!(c2["repeat_det"], Value::Null);
    assert_eq!(c2["judge"], Value::Null);
    assert_eq!(c2["graded_by"], "ungraded: judge off");
    let c1 = grades.iter().find(|g| g["case"] == "c:1").unwrap();
    assert_eq!(c1["graded_by"], "deterministic");
}
