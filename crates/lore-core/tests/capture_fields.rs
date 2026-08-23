//! F9 acceptance: `lore_captured` observations record what they claim to —
//! the HEAD commit subject, a size-capped changed-file summary, and bounded
//! ahead/behind counts relative to a tracking branch — via `gix` only.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::enrich::enrich_session;
use lore_core::ingest::persist_session;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

fn git_raw(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git must be installed to run capture-fields tests")
}

fn git(dir: &Path, args: &[&str]) {
    let output = git_raw(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "initial"]);
}

fn blobs() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    (dir, store)
}

/// A one-message Claude session whose recorded cwd is `cwd`.
fn persist_session_at(conn: &Connection, blobs: &BlobStore, dedupe: &str, cwd: &Path) -> String {
    let content = format!(
        "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"{dedupe}\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n",
        cwd.to_string_lossy()
    );
    let parsed = ClaudeCodeAdapter::new().parse_str(&content, dedupe);
    persist_session(conn, "claude-code", "Claude Code", &parsed, blobs).unwrap()
}

fn captured_row(
    conn: &Connection,
    sid: &str,
) -> (Option<String>, Option<String>, Option<i64>, Option<i64>) {
    conn.query_row(
        "SELECT commit_subject, changed_files_json, ahead, behind
         FROM git_observation WHERE session_id=?1 AND source='lore_captured'",
        [sid],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

#[test]
fn lore_captured_records_commit_subject_and_changed_files() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    // A dirty working tree: a modified tracked file and an untracked file.
    std::fs::write(repo.path().join("README.md"), "hello modified\n").unwrap();
    std::fs::write(repo.path().join("dirty.txt"), "uncommitted\n").unwrap();

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let sid = persist_session_at(&conn, &store, "cap", repo.path());
    enrich_session(&conn, &sid).unwrap();

    let (subject, changed, ahead, behind) = captured_row(&conn, &sid);
    assert_eq!(
        subject.as_deref(),
        Some("initial"),
        "lore_captured records the HEAD commit subject"
    );
    let changed = changed.expect("changed-file summary is written");
    assert!(
        changed.contains("dirty.txt"),
        "changed-file summary contains the dirty file: {changed}"
    );
    assert!(
        changed.contains("README.md"),
        "changed-file summary includes modified tracked files: {changed}"
    );
    assert_eq!(
        ahead, None,
        "no upstream -> ahead is NULL, not approximated"
    );
    assert_eq!(
        behind, None,
        "no upstream -> behind is NULL, not approximated"
    );
}

#[test]
fn lore_captured_records_ahead_behind_when_tracking_an_upstream() {
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remote.git");
    let local = root.path().join("local");

    // A bare remote, then a local clone tracking it.
    git(root.path(), &["init", "--bare", "remote.git"]);
    git(root.path(), &["init", "-b", "main", "local"]);
    std::fs::write(local.join("README.md"), "hello\n").unwrap();
    git(&local, &["add", "."]);
    git(&local, &["commit", "-m", "initial"]);
    git(
        &local,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&local, &["push", "-u", "origin", "main"]);

    // One unpushed commit: ahead=1, behind=0.
    std::fs::write(local.join("second.txt"), "more\n").unwrap();
    git(&local, &["add", "."]);
    git(&local, &["commit", "-m", "second"]);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let sid = persist_session_at(&conn, &store, "ab", &local);
    enrich_session(&conn, &sid).unwrap();

    let (subject, _changed, ahead, behind) = captured_row(&conn, &sid);
    assert_eq!(subject.as_deref(), Some("second"));
    assert_eq!(ahead, Some(1), "one unpushed commit is recorded as ahead");
    assert_eq!(
        behind,
        Some(0),
        "no unpulled commits are recorded as behind"
    );
}
