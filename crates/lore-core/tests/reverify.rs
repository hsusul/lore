//! M4 acceptance: re-verification records `lore_reverified` observations for
//! recorded commits/branches without ever overwriting the historical
//! `agent_recorded` rows — a rebased/GC'd commit is flagged missing, a deleted
//! branch is flagged gone, and a vanished checkout marks its worktree missing.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

use lore_core::adapters::codex::CodexAdapter;
use lore_core::enrich::{enrich_session, reverify_session};
use lore_core::ingest::persist_session;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

fn git(dir: &Path, args: &[&str]) {
    let out = git_raw(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

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
        .expect("git must be installed to run reverify tests")
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = git_raw(dir, args);
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
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

/// Persist a Codex session recording `commit`/branch main at `cwd`, then enrich.
fn persist_and_enrich(conn: &Connection, blobs: &BlobStore, cwd: &Path, commit: &str) -> String {
    let content = format!(
        concat!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{{\"id\":\"rv\",\"cli_version\":\"1\",\"cwd\":\"{cwd}\",\"model_provider\":\"openai\",\"git\":{{\"branch\":\"main\",\"commit_hash\":\"{commit}\"}}}}}}\n",
            "{{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"hi\"}}}}\n"
        ),
        cwd = cwd.to_string_lossy(),
        commit = commit
    );
    let parsed = CodexAdapter::new().parse_str(&content, "rv");
    let sid = persist_session(conn, "codex", "Codex", &parsed, blobs).unwrap();
    enrich_session(conn, &sid).unwrap();
    sid
}

fn count(conn: &Connection, sql: &str, sid: &str) -> i64 {
    conn.query_row(sql, [sid], |r| r.get(0)).unwrap()
}

#[test]
fn reverify_confirms_an_existing_commit_without_touching_history() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let sid = persist_and_enrich(&conn, &store, repo.path(), &head);

    let recorded = reverify_session(&conn, &sid).unwrap();
    assert_eq!(recorded, 1);

    // The agent-recorded row is untouched; a separate lore_reverified row is added.
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM git_observation WHERE session_id=?1 AND source='agent_recorded' AND commit_sha IS NOT NULL",
            &sid,
        ),
        1
    );
    let (exists, tconf): (i64, String) = conn
        .query_row(
            "SELECT commit_exists, temporal_confidence FROM git_observation
             WHERE session_id=?1 AND source='lore_reverified'",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(exists, 1, "the recorded commit still exists");
    assert_eq!(tconf, "retrospective");
}

#[test]
fn reverify_flags_a_missing_commit() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    // A syntactically valid sha that is not in the repo (as after a rebase/GC).
    let ghost = "0123456789abcdef0123456789abcdef01234567";

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let sid = persist_and_enrich(&conn, &store, repo.path(), ghost);
    reverify_session(&conn, &sid).unwrap();

    // agent_recorded still holds the original sha; reverified flags it missing.
    let recorded: String = conn
        .query_row(
            "SELECT commit_sha FROM git_observation WHERE session_id=?1 AND source='agent_recorded'",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(recorded, ghost, "history is preserved, not rewritten");
    let exists: i64 = conn
        .query_row(
            "SELECT commit_exists FROM git_observation WHERE session_id=?1 AND source='lore_reverified'",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists, 0, "a rebased/GC'd commit reverifies as missing");
}

#[test]
fn reverify_flags_a_deleted_branch_but_keeps_the_recorded_branch() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let sid = persist_and_enrich(&conn, &store, repo.path(), &head);

    // Delete branch main (detach first so it is not the current branch).
    git(repo.path(), &["checkout", "--detach", "HEAD"]);
    git(repo.path(), &["branch", "-D", "main"]);

    reverify_session(&conn, &sid).unwrap();

    // The recorded branch is retained on the agent_recorded row.
    let recorded_branch: String = conn
        .query_row(
            "SELECT branch FROM git_observation WHERE session_id=?1 AND source='agent_recorded'",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(recorded_branch, "main");

    // The commit still exists but the branch is gone — captured in metadata.
    let (exists, meta): (i64, String) = conn
        .query_row(
            "SELECT commit_exists, metadata_json FROM git_observation
             WHERE session_id=?1 AND source='lore_reverified'",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(exists, 1, "the commit object still exists");
    assert!(
        meta.contains("\"branch_exists\":false"),
        "deleted branch is flagged: {meta}"
    );
}

#[test]
fn reverify_marks_the_worktree_missing_when_the_checkout_is_gone() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let sid = persist_and_enrich(&conn, &store, repo.path(), &head);

    // The repository directory disappears.
    std::fs::remove_dir_all(repo.path()).unwrap();
    reverify_session(&conn, &sid).unwrap();

    let missing: i64 = conn
        .query_row("SELECT is_missing FROM worktree", [], |r| r.get(0))
        .unwrap();
    assert_eq!(missing, 1, "a vanished checkout marks its worktree missing");
    // Sessions and evidence are retained.
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM agent_session WHERE id=?1",
            &sid
        ),
        1
    );
    let exists: Option<i64> = conn
        .query_row(
            "SELECT commit_exists FROM git_observation WHERE session_id=?1 AND source='lore_reverified'",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        exists.is_none(),
        "commit existence is unknown when the repo is gone"
    );
}
