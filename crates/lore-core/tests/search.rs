//! M6 acceptance: FTS5 search over redacted projections — identifier/title/path
//! recall, provenance filters, highlighted snippets, and no secret ever
//! surfacing in a result snippet.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::codex::CodexAdapter;
use lore_core::ingest::persist_session;
use lore_core::search::{search, HIGHLIGHT_START};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

fn store() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    (dir, s)
}

fn user_message(session: &str, cwd: &str, text: &str) -> String {
    format!(
        "{{\"type\":\"user\",\"uuid\":\"u_{session}\",\"sessionId\":\"{session}\",\"cwd\":\"{cwd}\",\"message\":{{\"role\":\"user\",\"content\":\"{text}\"}}}}\n"
    )
}

fn persist_claude(conn: &Connection, blobs: &BlobStore, jsonl: &str, dedupe: &str) -> String {
    let parsed = ClaudeCodeAdapter::new().parse_str(jsonl, dedupe);
    persist_session(conn, "claude-code", "Claude Code", &parsed, blobs).unwrap()
}

#[test]
fn identifier_recall_with_highlighted_snippet() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    persist_claude(
        &conn,
        &blobs,
        &user_message("s1", "/p", "the retryBackoff helper handles jitter"),
        "s1",
    );

    let hits = search(&conn, "retryBackoff", 20).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source_kind, "message_part");
    assert!(hits[0].snippet.contains(HIGHLIGHT_START));
    assert!(hits[0].snippet.contains("retryBackoff"));

    // A term that is not present returns nothing.
    assert!(search(&conn, "nonexistentterm", 20).unwrap().is_empty());
    // An empty query returns nothing (never a bare FTS MATCH).
    assert!(search(&conn, "   ", 20).unwrap().is_empty());
}

#[test]
fn title_is_searchable() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let jsonl = format!(
        "{}{}",
        "{\"type\":\"custom-title\",\"sessionId\":\"t1\",\"customTitle\":\"Fix billing webhook signature\"}\n",
        user_message("t1", "/p", "hello there")
    );
    persist_claude(&conn, &blobs, &jsonl, "t1");

    let hits = search(&conn, "webhook", 20).unwrap();
    assert!(hits.iter().any(|h| h.field == "title"));
}

#[test]
fn agent_filter_narrows_results() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    persist_claude(
        &conn,
        &blobs,
        &user_message("c1", "/p", "deploy the service"),
        "c1",
    );
    let codex = concat!(
        "{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"id\":\"x1\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}\n",
        "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"deploy the service\"}}\n"
    );
    let parsed = CodexAdapter::new().parse_str(codex, "x1");
    persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();

    assert_eq!(search(&conn, "deploy", 20).unwrap().len(), 2);
    let codex_only = search(&conn, "deploy agent:codex", 20).unwrap();
    assert_eq!(codex_only.len(), 1);
    assert_eq!(codex_only[0].agent_id, "codex");
}

#[test]
fn path_filter_matches_touched_files() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // A session whose assistant edits auth/login.ts and whose text says "the fix".
    let jsonl = format!(
        "{}{}",
        user_message("p1", "/p", "here is the fix"),
        "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"p1\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Edit\",\"input\":{\"file_path\":\"auth/login.ts\"}}]}}\n"
    );
    persist_claude(&conn, &blobs, &jsonl, "p1");

    assert_eq!(search(&conn, "fix path:auth/", 20).unwrap().len(), 1);
    assert!(search(&conn, "fix path:billing/", 20).unwrap().is_empty());
}

#[test]
fn has_error_filter_selects_sessions_with_a_failed_tool() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // e1: text mentions "migration" and has a failing tool call.
    let with_error = format!(
        "{}{}{}",
        user_message("e1", "/p", "the migration failed here"),
        "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"e1\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Bash\",\"input\":{\"command\":\"npm test\"}}]}}\n",
        "{\"type\":\"user\",\"uuid\":\"u2\",\"sessionId\":\"e1\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t1\",\"is_error\":true,\"content\":\"1 failing\"}]}}\n"
    );
    persist_claude(&conn, &blobs, &with_error, "e1");
    // e2: mentions "migration" but no failure.
    persist_claude(
        &conn,
        &blobs,
        &user_message("e2", "/p", "the migration succeeded"),
        "e2",
    );

    assert_eq!(search(&conn, "migration", 20).unwrap().len(), 2);
    let failed = search(&conn, "migration has:error", 20).unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(
        failed[0].session_id,
        search(&conn, "failed has:error", 20).unwrap()[0].session_id
    );
}

#[test]
fn snippets_never_surface_a_secret() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let secret = format!("ghp{}", "_0123456789abcdefghijklmnopqrstuvwxyz");
    let body = "0123456789abcdefghijklmnopqrstuvwxyz";
    persist_claude(
        &conn,
        &blobs,
        &user_message("k1", "/p", &format!("deploy with token {secret} now")),
        "k1",
    );

    let hits = search(&conn, "deploy", 20).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("deploy"));
    assert!(!hits[0].snippet.contains(&secret));
    assert!(
        !hits[0].snippet.contains(body),
        "no raw secret in a snippet"
    );
}
