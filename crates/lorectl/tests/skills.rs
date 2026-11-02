//! `lorectl skills` as a process, against the lore-skills fixture home. HOME is
//! always pointed at a fixture or a temp dir, never the real one.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

fn fixture_home() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../lore-skills/fixtures/home")
}

fn command(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lorectl"));
    command.args(args).env("HOME", home);
    command
}

fn lorectl(home: &Path, args: &[&str]) -> Output {
    command(home, args).output().expect("lorectl runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf-8")
}

fn json(output: &Output) -> Vec<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(&stdout(output)).unwrap();
    value.as_array().unwrap().clone()
}

#[test]
fn ls_json_lists_one_record_per_distinct_content() {
    let output = lorectl(&fixture_home(), &["--json", "skills", "ls"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json(&output).len(), 13);
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
    for args in [&["skills", "ls"][..], &["--json", "skills", "ls"]] {
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
    let output = lorectl(Path::new("relative/home"), &["skills", "ls"]);
    assert_eq!(output.status.code(), Some(1));
}
