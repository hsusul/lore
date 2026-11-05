//! `lore-replay sweep` against a fake `claude`: screening, resume, and the
//! stop conditions. The fake never reaches a model provider.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{json, Value};

/// Says `$FAKE_TEXT` (default: a turn that repeats the mistake) for 0.12, or
/// the usage-limit error once more than `$LIMIT_AFTER` calls were made.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
cat > /dev/null
echo call >> "$CALLS"
n=$(wc -l < "$CALLS" | tr -d ' ')
if [ -n "$LIMIT_AFTER" ] && [ "$n" -gt "$LIMIT_AFTER" ]; then
  echo '{"type":"result","is_error":true,"num_turns":0,"total_cost_usd":0,"result":"You'"'"'ve hit your 5-hour limit"}'
  exit 1
fi
text="${FAKE_TEXT:-I will REPEAT it}"
echo "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"$text\"}]}}"
echo '{"type":"result","is_error":false,"num_turns":1,"total_cost_usd":0.12}'
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn calls(&self) -> usize {
        std::fs::read_to_string(self.root.join("calls"))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    fn out(&self) -> PathBuf {
        self.root.join("runs")
    }

    fn screen(&self) -> Vec<Value> {
        std::fs::read_to_string(self.out().join("screen.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn runs(&self, case: &str) -> Vec<String> {
        let dir = self.out().join(case);
        let mut found: Vec<String> = std::fs::read_dir(&dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.path().join("run.json").is_file())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        found
    }
}

fn fixture(cases: &[&str], arms: &Value) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let session = root.join("native-1.jsonl");
    let lines = [
        json!({"type": "user", "sessionId": "native-1", "message": {"content": "start"}}),
        json!({"type": "assistant", "sessionId": "native-1",
               "message": {"content": [{"type": "text", "text": "done"}]}}),
        json!({"type": "user", "sessionId": "native-1", "message": {"content": "now fix it"}}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&session, text).unwrap();
    let mut labeled = String::new();
    for id in cases {
        let line = json!({
            "id": id, "lesson": "l", "session": "s", "native": "native-1", "model": null,
            "file": session, "prompt_line": 2, "correction_line": 4, "correction_ms": 2,
            "cwd": null, "branch": null, "prompt_ms": 1, "prompt": "now fix it",
            "actions": [], "correction": "no", "repo": null, "signal": "correction",
            "label": {"mistake": true, "visible": true, "what": "repeats", "lesson": "don't",
                      "grader": {"kind": "text_pattern", "pattern": "REPEAT"}},
            "labeled_by": "judge: claude sonnet"
        });
        labeled.push_str(&format!("{line}\n"));
    }
    std::fs::write(root.join("labeled.jsonl"), labeled).unwrap();
    std::fs::write(root.join("arms.json"), arms.to_string()).unwrap();
    let fake = root.join("fake-claude");
    std::fs::write(&fake, FAKE_CLAUDE).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir_all(root.join("home")).unwrap();
    Fixture { _tmp: tmp, root }
}

fn sweep(f: &Fixture, cases: &[&str], extra: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lore-replay"));
    cmd.arg("sweep")
        .arg("--cases")
        .arg(f.root.join("labeled.jsonl"))
        .arg("--arms")
        .arg(f.root.join("arms.json"))
        .arg("--out")
        .arg(f.out());
    for case in cases {
        cmd.args(["--case", case]);
    }
    cmd.args(extra)
        .env("HOME", f.root.join("home"))
        .env("CLAUDE_CONFIG_DIR", f.root.join("claude"))
        .env("LORE_REPLAY_CLAUDE", f.root.join("fake-claude"))
        .env("CALLS", f.root.join("calls"))
        .env_remove("FAKE_TEXT")
        .env_remove("LIMIT_AFTER");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn arms_for(case: &str) -> Value {
    json!({case: {"file": "", "lore": "LORE", "oracle": "ORACLE"}})
}

#[test]
fn a_reproducing_case_runs_every_arm_and_resumes() {
    let f = fixture(&["c:1"], &arms_for("c:1"));
    let out = sweep(&f, &["c:1"], &["--trials", "2", "--jobs", "2"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.runs("c_1"),
        [
            "lore-1", "lore-2", "none-0", "none-1", "none-2", "oracle-1", "oracle-2", "own-1",
            "own-2"
        ]
    );
    let screen = f.screen();
    assert_eq!(screen[0]["case"], "c:1");
    assert_eq!(screen[0]["verdict"], "YES");
    assert_eq!(screen[1]["arm"], "file");
    assert_eq!(screen[1]["same_as_none"], true);
    assert!(stdout(&out).contains("sweep: 9 runs made, 0 already done; 0 of 1 screened"));
    assert!(stdout(&out).contains("$1.08 spent"));
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(f.out().join("c_1/none-0/run.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["harness"], 2);

    // A rerun keeps every finished run and screens nothing again.
    let again = sweep(&f, &["c:1"], &["--trials", "2"], &[]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(f.calls(), 9);
    assert!(stdout(&again).contains("sweep: 0 runs made, 9 already done"));
    assert_eq!(f.screen().len(), 2);
}

#[test]
fn a_case_that_does_not_reproduce_stops_at_screening() {
    let f = fixture(
        &["c:1", "c:2"],
        &json!({"c:1": {"oracle": "O"}, "c:2": {"oracle": "O"}}),
    );
    let env = [("FAKE_TEXT", "Done, the other way")];
    let out = sweep(&f, &["c:1", "c:2"], &[], &env);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.runs("c_1"), ["none-0"]);
    assert_eq!(f.runs("c_2"), ["none-0"]);
    let screen = f.screen();
    assert_eq!(screen.len(), 2);
    assert!(screen.iter().all(|l| l["verdict"] == "NO"));
    assert!(screen[0]["reason"]
        .as_str()
        .unwrap()
        .contains("does not repeat"));
    assert!(stdout(&out).contains("2 of 2 screened cases screened out"));

    // Screened out stays out, even when the agent would now repeat.
    let list = f.root.join("cases.txt");
    std::fs::write(&list, "# the two cases\nc:1\n\nc:2\n").unwrap();
    let list = list.display().to_string();
    let again = sweep(&f, &[], &["--case-list", &list], &[]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(f.calls(), 2);
}

#[test]
fn a_usage_limit_stops_the_whole_sweep() {
    let f = fixture(
        &["c:1", "c:2"],
        &json!({"c:1": {"oracle": "O"}, "c:2": {"oracle": "O"}}),
    );
    let out = sweep(
        &f,
        &["c:1", "c:2"],
        &["--jobs", "1"],
        &[("LIMIT_AFTER", "2")],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("hit the usage limit"));
    assert!(stderr(&out).contains("hit your 5-hour limit"));
    assert_eq!(f.calls(), 3, "no run may start after the limit");
    assert!(f.runs("c_2").is_empty());
    assert!(stdout(&out).contains("sweep: 3 runs made"));
}

#[test]
fn max_cost_stops_the_sweep() {
    let f = fixture(&["c:1"], &json!({"c:1": {"oracle": "O"}}));
    let out = sweep(&f, &["c:1"], &["--max-cost", "0.3"], &[]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(stderr(&out).contains("over --max-cost $0.30"));
    // 0.12 a run: the fourth would start with 0.36 spent.
    assert_eq!(f.calls(), 3);
    assert_eq!(f.runs("c_1"), ["none-0", "none-1", "none-2"]);
}

#[test]
fn a_missing_oracle_text_fails_before_any_run() {
    let f = fixture(&["c:1", "c:2"], &json!({"c:1": {"oracle": "O"}, "c:2": {}}));
    let out = sweep(&f, &["c:1", "c:2"], &[], &[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no `oracle` text for case `c:2`"));
    assert_eq!(f.calls(), 0);
}
