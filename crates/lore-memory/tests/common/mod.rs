//! Scratch git repositories for the anchor, scope, and recall tests.
#![allow(clippy::expect_used, clippy::unwrap_used, dead_code)]

use std::fs;
use std::path::Path;
use std::process::Command;

/// Run git in `dir` with no user or system config, and return stdout.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@localhost"])
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A new repository at `dir` with `origin` set to `remote`, if any.
pub fn repo(dir: &Path, remote: Option<&str>) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    if let Some(remote) = remote {
        git(dir, &["remote", "add", "origin", remote]);
    }
}

/// Write `text` to `rel` in `dir` and commit everything. Returns the commit.
pub fn commit(dir: &Path, rel: &str, text: &str, message: &str) -> String {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}
