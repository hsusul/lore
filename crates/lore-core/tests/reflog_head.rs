//! Enrichment records the commit HEAD pointed at when a segment began, read from
//! the repository's HEAD reflog, as a `lore_reflog` observation dated to that
//! moment. A segment older than the reflog gets none.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::enrich::enrich_session;
use lore_core::ingest::persist_session;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

/// Run git in `dir` with every ref update stamped at `epoch` seconds.
fn git_at(dir: &Path, epoch: i64, args: &[&str]) -> String {
    let date = format!("{epoch} +0000");
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .expect("git must be installed");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A repository whose HEAD moved to `first` at 1_000_000_000 s and to `second`
/// at 1_000_000_600 s.
fn repo_with_history(dir: &Path) -> (String, String) {
    git_at(dir, 1_000_000_000, &["init", "-q", "-b", "main"]);
    git_at(
        dir,
        1_000_000_000,
        &["commit", "-q", "--allow-empty", "-m", "first"],
    );
    let first = git_at(dir, 1_000_000_000, &["rev-parse", "HEAD"]);
    git_at(
        dir,
        1_000_000_600,
        &["commit", "-q", "--allow-empty", "-m", "second"],
    );
    let second = git_at(dir, 1_000_000_600, &["rev-parse", "HEAD"]);
    (first, second)
}

/// A one-message Claude session in `cwd`, its message written at `iso`.
fn session_at(conn: &Connection, blobs: &BlobStore, id: &str, cwd: &Path, iso: &str) -> String {
    let content = format!(
        "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"{id}\",\"timestamp\":\"{iso}\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n",
        cwd.to_string_lossy()
    );
    let parsed = ClaudeCodeAdapter::new().parse_str(&content, id);
    persist_session(conn, "claude-code", "Claude Code", &parsed, blobs).unwrap()
}

fn reflog_rows(conn: &Connection, sid: &str) -> Vec<(String, String, i64, Option<String>)> {
    let mut stmt = conn
        .prepare(
            "SELECT commit_sha, temporal_confidence, event_ts, metadata_json
             FROM git_observation WHERE session_id = ?1 AND source = 'lore_reflog'",
        )
        .unwrap();
    stmt.query_map([sid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn records_the_commit_head_pointed_at_when_the_segment_began() {
    let repo = tempfile::tempdir().unwrap();
    let (first, second) = repo_with_history(repo.path());
    let path = repo.path().canonicalize().unwrap();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();

    // 1_000_000_300 s is 2001-09-09T01:51:40Z, between the two commits.
    let sid = session_at(&conn, &blobs, "between", &path, "2001-09-09T01:51:40.000Z");
    enrich_session(&conn, &sid).unwrap();
    let rows = reflog_rows(&conn, &sid);
    assert_eq!(rows.len(), 1);
    let (sha, confidence, event_ts, metadata) = &rows[0];
    assert_eq!(
        sha, &first,
        "HEAD at session start, not HEAD now ({second})"
    );
    assert_eq!(confidence, "exact_event");
    assert_eq!(*event_ts, 1_000_000_300_000);
    assert!(metadata
        .as_deref()
        .unwrap()
        .contains("\"method\":\"reflog\""));

    let captured: String = conn
        .query_row(
            "SELECT commit_sha FROM git_observation WHERE session_id = ?1 AND source = 'lore_captured'",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        captured, second,
        "the current_only capture still reads HEAD now"
    );
}

#[test]
fn records_nothing_for_a_segment_older_than_the_reflog() {
    let repo = tempfile::tempdir().unwrap();
    repo_with_history(repo.path());
    let path = repo.path().canonicalize().unwrap();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();

    let sid = session_at(&conn, &blobs, "older", &path, "2000-01-01T00:00:00.000Z");
    assert_eq!(
        enrich_session(&conn, &sid).unwrap(),
        1,
        "the segment still links"
    );
    assert!(reflog_rows(&conn, &sid).is_empty());
}

#[test]
fn a_second_enrichment_keeps_the_first_reading() {
    let repo = tempfile::tempdir().unwrap();
    let (first, _) = repo_with_history(repo.path());
    let path = repo.path().canonicalize().unwrap();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();

    let sid = session_at(&conn, &blobs, "twice", &path, "2001-09-09T01:51:40.000Z");
    enrich_session(&conn, &sid).unwrap();
    conn.execute(
        "UPDATE session_segment SET repository_id = NULL WHERE session_id = ?1",
        [&sid],
    )
    .unwrap();
    std::fs::remove_dir_all(path.join(".git/logs")).unwrap();
    enrich_session(&conn, &sid).unwrap();
    let rows = reflog_rows(&conn, &sid);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].0, first,
        "an expired reflog does not erase the reading"
    );
}
