//! M6 acceptance: cleartext is secret-scanned at ingest. Planted secrets are
//! recorded as findings and never reach the SearchDocument/FTS projections;
//! thinking is scanned but not indexed; a patch with a secret quarantines-by-
//! finding and redacts its projection; re-ingest does not duplicate.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::codex::CodexAdapter;
use lore_core::ingest::persist_session;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

/// A synthetic github-token, assembled from split parts (never a contiguous
/// literal, so it cannot trip upstream push-protection on this fixture).
fn secret() -> String {
    format!("ghp{}", "_0123456789abcdefghijklmnopqrstuvwxyz")
}

/// The alphanumeric body of the secret, the part FTS would tokenize.
fn secret_body() -> String {
    "0123456789abcdefghijklmnopqrstuvwxyz".to_string()
}

fn store() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    (dir, s)
}

fn persist_claude(conn: &Connection, blobs: &BlobStore, jsonl: &str) -> String {
    let parsed = ClaudeCodeAdapter::new().parse_str(jsonl, "sec");
    persist_session(conn, "claude-code", "Claude Code", &parsed, blobs).unwrap()
}

fn fts_matches(conn: &Connection, query: &str) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM search_fts WHERE search_fts MATCH ?1",
        [query],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn message_text_secret_is_found_and_never_indexed() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let content = format!("deploy with token {} tonight", secret());
    let jsonl = format!(
        "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"sec\",\"cwd\":\"/p\",\"message\":{{\"role\":\"user\",\"content\":\"{content}\"}}}}\n"
    );
    let sid = persist_claude(&conn, &blobs, &jsonl);

    // The secret is recorded as a finding (offsets + rule, no cleartext copy).
    let (findings, rule): (i64, String) = conn
        .query_row(
            "SELECT count(*), min(rule) FROM secret_finding
             WHERE session_id=?1 AND source_kind='message_part'",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(findings, 1);
    assert_eq!(rule, "github-token");

    // The projection is redacted: surrounding words present, secret absent.
    let redacted: String = conn
        .query_row(
            "SELECT redacted_text FROM search_document
             WHERE session_id=?1 AND source_kind='message_part'",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(redacted.contains("deploy") && redacted.contains("tonight"));
    assert!(!redacted.contains(&secret()));
    assert!(!redacted.contains(&secret_body()));

    // FTS finds the context but not the secret body.
    assert!(fts_matches(&conn, "deploy") >= 1);
    assert_eq!(fts_matches(&conn, &secret_body()), 0);
}

#[test]
fn thinking_secret_is_scanned_but_not_indexed() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // Assistant turn: a thinking block carrying a secret, then plain text.
    let thinking = format!("i will use {}", secret());
    let jsonl = format!(
        "{{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"sec\",\"cwd\":\"/p\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"thinking\",\"thinking\":\"{thinking}\"}},{{\"type\":\"text\",\"text\":\"done deploying\"}}]}}}}\n"
    );
    let sid = persist_claude(&conn, &blobs, &jsonl);

    // The thinking secret is still found...
    let findings: i64 = conn
        .query_row(
            "SELECT count(*) FROM secret_finding WHERE session_id=?1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(findings >= 1);

    // ...but no SearchDocument (and no FTS entry) contains it: thinking is not
    // indexed, and the only indexed part is the plain "done deploying" text.
    let leaked: i64 = conn
        .query_row(
            "SELECT count(*) FROM search_document
             WHERE session_id=?1 AND redacted_text LIKE '%' || ?2 || '%'",
            rusqlite::params![sid, secret_body()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(leaked, 0);
    assert_eq!(fts_matches(&conn, &secret_body()), 0);
    assert!(fts_matches(&conn, "deploying") >= 1);
}

#[test]
fn recorded_patch_secret_quarantines_blob_and_redacts_projection() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // A Codex apply_patch whose recorded content includes a secret.
    let content = format!("const KEY = {}", secret());
    let jsonl = format!(
        concat!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{{\"id\":\"p\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}}}\n",
            "{{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{{\"type\":\"function_call\",\"name\":\"apply_patch\",\"arguments\":\"{{}}\",\"call_id\":\"c1\"}}}}\n",
            "{{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:02.000Z\",\"payload\":{{\"type\":\"patch_apply_end\",\"call_id\":\"c1\",\"success\":true,\"changes\":{{\"config.ts\":{{\"type\":\"add\",\"content\":\"{content}\"}}}}}}}}\n"
        ),
        content = content
    );
    let parsed = CodexAdapter::new().parse_str(&jsonl, "p");
    let sid = persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();

    // The patch blob is finalized as `findings` (not clean).
    let state: String = conn
        .query_row(
            "SELECT b.scan_state FROM file_event f JOIN blob b ON b.id=f.patch_blob_id
             WHERE f.session_id=?1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "findings");

    // A finding is recorded against the file event, and the projection redacts.
    let findings: i64 = conn
        .query_row(
            "SELECT count(*) FROM secret_finding WHERE session_id=?1 AND source_kind='file_event'",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(findings, 1);
    assert_eq!(fts_matches(&conn, &secret_body()), 0);
}

#[test]
fn reingest_does_not_duplicate_projections_or_findings() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let content = format!("token {} end", secret());
    let jsonl = format!(
        "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"sec\",\"cwd\":\"/p\",\"message\":{{\"role\":\"user\",\"content\":\"{content}\"}}}}\n"
    );
    persist_claude(&conn, &blobs, &jsonl);
    persist_claude(&conn, &blobs, &jsonl);

    let (docs, findings, fts): (i64, i64, i64) = conn
        .query_row(
            "SELECT
                (SELECT count(*) FROM search_document),
                (SELECT count(*) FROM secret_finding),
                (SELECT count(*) FROM search_fts)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(docs, 1, "one message part projection, not duplicated");
    assert_eq!(findings, 1);
    assert_eq!(fts, 1, "FTS stays consistent with its content table");
}
