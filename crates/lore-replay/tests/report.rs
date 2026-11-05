//! `lore-replay report` on a small synthetic grades file.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

fn write_lines(path: &Path, lines: &[Value]) {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(path, text).unwrap();
}

/// Four included cases and one excluded. `none` repeats nearly always,
/// `oracle` almost never, `lore` the same as none, and `file` is empty for
/// case c:3.
fn fixture(dir: &Path) {
    let label = |id: &str, mistake: bool, visible: bool| {
        json!({"id": id, "correction": "no", "labeled_by": "judge: claude sonnet",
               "label": {"mistake": mistake, "visible": visible, "what": format!("mistake {id}"),
                         "lesson": "l", "grader": {"kind": "judge", "rubric": "r"}}})
    };
    write_lines(
        &dir.join("labeled.jsonl"),
        &[
            label("c:0", true, true),
            label("c:1", true, true),
            label("c:2", true, true),
            label("c:3", true, true),
            label("c:x", true, false),
        ],
    );
    let mut grades = Vec::new();
    let mut add = |case: &str, arm: &str, verdicts: &[&str]| {
        for (t, v) in verdicts.iter().enumerate() {
            grades.push(json!({"case": case, "arm": arm, "trial": t + 1, "grader": "judge",
                               "repeat_det": null, "judge": v, "graded_by": "judge: claude sonnet"}));
        }
    };
    for case in ["c:0", "c:1", "c:2", "c:3"] {
        add(case, "none", &["YES", "YES", "YES"]);
        add(case, "oracle", &["NO", "NO", "NO"]);
        add(case, "lore", &["YES", "YES", "NO"]);
        add(case, "file", &["YES", "NO", "UNKNOWN"]);
    }
    add("c:x", "none", &["NO", "NO", "NO"]);
    write_lines(&dir.join("grades.jsonl"), &grades);
    write_lines(
        &dir.join("shown.jsonl"),
        &[
            json!({"case": "c:0", "arm": "lore", "shown": "YES", "reason": "r", "judged_by": "j"}),
            json!({"case": "c:1", "arm": "lore", "shown": "NO", "reason": "r", "judged_by": "j"}),
        ],
    );
    std::fs::write(
        dir.join("arms.json"),
        json!({"c:3": {"file": "", "lore": "x", "oracle": "y"}}).to_string(),
    )
    .unwrap();
}

fn report(dir: &Path, extra: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_lore-replay"))
        .current_dir(dir)
        .args([
            "report",
            "--labels",
            "labeled.jsonl",
            "--grades",
            "grades.jsonl",
            "--shown",
            "shown.jsonl",
            "--arms",
            "arms.json",
            "--data",
            "synthetic",
        ])
        .args(extra)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn the_report_implements_the_preregistered_analysis() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let text = report(tmp.path(), &[]);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[1].starts_with("Stop rule: none repeats the mistake in 12 of 12"));
    assert!(text.contains("Cases: 4 of 5 labeled cases included"));
    assert!(text.contains("4 UNKNOWN or unjudged, excluded; 3 on excluded cases"));
    assert!(text.contains("none    100% [100%, 100%] (n=4 cases, 12 trials)"));
    assert!(text.contains(
        "oracle  0% [0%, 0%] (n=4 cases, 12 trials); vs none -100 points [-100, -100] over n=4 paired cases"
    ));
    assert!(text.contains("same as none for 1 case(s) whose file context is empty"));
    assert!(text.contains("c:3  none 3/3  file 3/3=none  lore 2/3  oracle 0/3"));
    assert!(text.contains("lore: context holds a relevant lesson for 1 of 2 (50%) judged cases"));
    // Every line with a number says where it was measured.
    for line in lines
        .iter()
        .filter(|l| l.chars().any(|c| c.is_ascii_digit()))
    {
        assert!(line.contains("synthetic data"), "untagged: {line}");
    }

    let json: Value = serde_json::from_str(&report(tmp.path(), &["--json"])).unwrap();
    assert_eq!(json["data"], "synthetic");
    assert_eq!(json["stop"]["can_measure"], true);
    let oracle = json["arms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["arm"] == "oracle")
        .unwrap();
    assert_eq!(oracle["vs_none"]["measurable"], true);
    assert_eq!(oracle["rate"]["trials"], 12);

    // The seed fixes the intervals.
    assert_eq!(
        report(tmp.path(), &["--seed", "9"]),
        report(tmp.path(), &["--seed", "9"])
    );
}
