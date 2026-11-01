//! `lorectl skills` as a process, against the lore-skills fixture home. HOME is
//! always pointed at a fixture or a temp dir, never the real one.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

fn fixture_home() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../lore-skills/fixtures/home")
}

fn lorectl(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .env("HOME", home)
        .output()
        .expect("lorectl runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf-8")
}

#[test]
fn doctor_reports_and_still_exits_zero() {
    let output = lorectl(&fixture_home(), &["skills", "doctor"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    let summary = text.lines().last().unwrap();
    assert_eq!(
        summary,
        "2 duplicates, 1 drift, 6 unknown sources, 4 broken refs, 1 name mismatches, \
         1 bad frontmatter, 1 codex budget; 16 findings; 9 skills found"
    );
    assert!(text.starts_with("bad-yaml\n  bad-frontmatter"), "{text}");
}

#[test]
fn doctor_json_is_the_same_findings_each_with_a_fix() {
    let output = lorectl(&fixture_home(), &["skills", "doctor", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let findings: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let findings = findings.as_array().unwrap();
    assert_eq!(findings.len(), 16);
    assert_eq!(findings[0]["kind"], "bad-frontmatter");
    assert!(findings
        .iter()
        .all(|f| f["fix"].as_str().is_some_and(|s| !s.is_empty())));
}

#[test]
fn ls_json_lists_one_record_per_distinct_content() {
    let output = lorectl(&fixture_home(), &["--json", "skills", "ls"]);
    assert_eq!(output.status.code(), Some(0));
    let records: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 10);
}

#[test]
fn an_empty_home_finds_nothing_and_exits_zero() {
    let home = tempfile::tempdir().unwrap();
    let output = lorectl(home.path(), &["skills", "doctor"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output)
        .trim_end()
        .ends_with("0 findings; 0 skills found"));
}

/// Every path below `root` with its modification time and length.
fn snapshot(root: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, SystemTime, u64)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            out.push((path.clone(), meta.modified().unwrap(), meta.len()));
            if meta.is_dir() {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

#[test]
fn skills_commands_write_nothing_under_home() {
    let home = fixture_home();
    let before = snapshot(&home);
    for args in [
        &["skills", "doctor"][..],
        &["skills", "doctor", "--json"],
        &["skills", "ls"],
    ] {
        assert_eq!(lorectl(&home, args).status.code(), Some(0));
    }
    assert_eq!(snapshot(&home), before);
}

#[test]
fn a_missing_or_unknown_subcommand_is_a_usage_error() {
    let home = fixture_home();
    for args in [&["skills"][..], &["skills", "fix"]] {
        let output = lorectl(&home, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
    }
}

#[test]
fn a_relative_home_is_a_usage_error() {
    let output = lorectl(Path::new("relative/home"), &["skills", "doctor"]);
    assert_eq!(output.status.code(), Some(1));
}
