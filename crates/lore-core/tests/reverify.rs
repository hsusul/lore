//! M4 acceptance: re-verification records `lore_reverified` observations for
//! recorded commits/branches without ever overwriting the historical
//! `agent_recorded` rows — a rebased/GC'd commit is flagged missing, a deleted
//! branch is flagged gone, and a vanished checkout marks its worktree missing.
//!
//! It also pins the two properties that make the observations *evidence* rather
//! than a status light: a changed verdict **appends** a row (so the transition
//! stays recoverable) while an unchanged verdict refreshes one in place, and a
//! repository that is present but unreadable is treated as transient rather
//! than flagging a live worktree missing.
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
fn git_snapshot_returns_all_three_provenances_labeled() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let sid = persist_and_enrich(&conn, &store, repo.path(), &head);
    reverify_session(&conn, &sid).unwrap();

    let snapshot = lore_core::query::get_git_snapshot(&conn, &sid).unwrap();
    let sources: Vec<&str> = snapshot.iter().map(|o| o.source.as_str()).collect();
    assert!(sources.contains(&"agent_recorded"));
    assert!(sources.contains(&"lore_captured"));
    assert!(sources.contains(&"lore_reverified"));

    // The agent-recorded row is near_event; the captured row never claims
    // session-time; the reverified row is retrospective and confirms the commit.
    let recorded = snapshot
        .iter()
        .find(|o| o.source == "agent_recorded")
        .unwrap();
    assert_eq!(recorded.temporal_confidence, "near_event");
    assert_eq!(recorded.commit_sha.as_deref(), Some(head.as_str()));

    let captured = snapshot
        .iter()
        .find(|o| o.source == "lore_captured")
        .unwrap();
    assert_eq!(captured.temporal_confidence, "current_only");

    let reverified = snapshot
        .iter()
        .find(|o| o.source == "lore_reverified")
        .unwrap();
    assert_eq!(reverified.temporal_confidence, "retrospective");
    assert_eq!(reverified.commit_exists, Some(true));
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
fn reverify_appends_a_row_when_the_verdict_changes_and_refreshes_when_it_does_not() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let sid = persist_and_enrich(&conn, &store, repo.path(), &head);

    let reverified = "SELECT count(*) FROM git_observation
                      WHERE session_id=?1 AND source='lore_reverified'";

    // First pass: the commit exists and so does its branch.
    reverify_session(&conn, &sid).unwrap();
    assert_eq!(count(&conn, reverified, &sid), 1);

    // Re-running with nothing changed must NOT append: the verdict is part of
    // the observation id, so the row is refreshed in place.
    reverify_session(&conn, &sid).unwrap();
    assert_eq!(
        count(&conn, reverified, &sid),
        1,
        "an unchanged verdict refreshes its row instead of appending"
    );

    // The branch is deleted (as after a merge): the verdict changes.
    git(repo.path(), &["checkout", "--detach", "HEAD"]);
    git(repo.path(), &["branch", "-D", "main"]);
    reverify_session(&conn, &sid).unwrap();

    assert_eq!(
        count(&conn, reverified, &sid),
        2,
        "a changed verdict appends a second observation instead of overwriting"
    );

    // Both verdicts are recoverable, and the earlier one kept its own
    // observation time — this is the transition the audit found was being lost.
    let mut stmt = conn
        .prepare(
            "SELECT metadata_json, observed_at FROM git_observation
             WHERE session_id=?1 AND source='lore_reverified'
             ORDER BY observed_at, id",
        )
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([&sid], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .any(|(meta, _)| meta.contains("\"branch_exists\":true")),
        "the original 'branch present' verdict survives: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|(meta, _)| meta.contains("\"branch_exists\":false")),
        "the new 'branch gone' verdict is recorded: {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|(meta, _)| meta.contains("last_checked_at")),
        "every observation records when it was last confirmed: {rows:?}"
    );

    // The agent-recorded row is still untouched by any of this.
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM git_observation
             WHERE session_id=?1 AND source='agent_recorded'",
            &sid
        ),
        1
    );
}

#[test]
fn an_unreadable_repository_does_not_mark_the_worktree_missing() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let sid = persist_and_enrich(&conn, &store, repo.path(), &head);

    // The checkout is still there, but the repository cannot be opened. A
    // dangling `gitdir:` pointer reproduces this deterministically on every
    // platform, unlike a permission change (which root would bypass).
    std::fs::remove_dir_all(repo.path().join(".git")).unwrap();
    std::fs::write(repo.path().join(".git"), "gitdir: /nonexistent/lore-test\n").unwrap();

    reverify_session(&conn, &sid).unwrap();

    let missing: i64 = conn
        .query_row("SELECT is_missing FROM worktree", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        missing, 0,
        "a present-but-unreadable repository is transient and must not flag the worktree missing"
    );

    let meta: String = conn
        .query_row(
            "SELECT metadata_json FROM git_observation
             WHERE session_id=?1 AND source='lore_reverified'",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        meta.contains("repository_unreadable"),
        "the reason is still recorded as evidence: {meta}"
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
