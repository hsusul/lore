//! M1 acceptance: a parsed Claude session round-trips into SQLite faithfully —
//! ordered mixed content, thinking metadata, tool results, segments on cwd
//! change, out-of-order parent resolution, and idempotent re-ingest.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::codex::CodexAdapter;
use lore_core::ingest::persist_session;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

fn fixture_in(dir: &str, name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(dir)
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn fixture(name: &str) -> String {
    fixture_in("fixtures/claude_code", name)
}

/// A blob store rooted in a fresh temp dir; the returned guard must stay in
/// scope so the blob files survive for the duration of the test.
fn blob_store() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    (dir, store)
}

fn persist_fixture(conn: &Connection, blobs: &BlobStore, name: &str) -> String {
    let parsed = ClaudeCodeAdapter::new().parse_str(&fixture(name), "fallback");
    persist_session(conn, "claude-code", "Claude Code", &parsed, blobs).unwrap()
}

fn persist_codex(conn: &Connection, blobs: &BlobStore, name: &str) -> String {
    let parsed = CodexAdapter::new().parse_str(&fixture_in("fixtures/codex", name), "fallback");
    persist_session(conn, "codex", "Codex", &parsed, blobs).unwrap()
}

#[test]
fn basic_session_round_trips_in_order() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blob_store();
    let sid = persist_fixture(&conn, &blobs, "basic_text.jsonl");

    let (count, title, input, output, cache): (i64, String, i64, i64, i64) = conn
        .query_row(
            "SELECT message_count, title, total_input_tokens, total_output_tokens,
                    total_cache_tokens
             FROM agent_session WHERE id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(title, "Add health check endpoint");
    assert_eq!((input, output, cache), (1200, 40, 800));

    // Messages in source order.
    let roles: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT role FROM message WHERE session_id = ?1 ORDER BY seq")
            .unwrap();
        let rows = stmt
            .query_map([&sid], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows
    };
    assert_eq!(roles, vec!["user", "assistant"]);

    // Assistant parts ordered thinking(non-searchable) then text(searchable).
    let parts: Vec<(String, i64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT mp.kind, mp.searchable FROM message_part mp
                 JOIN message m ON m.id = mp.message_id
                 WHERE m.session_id = ?1 AND m.seq = 1 ORDER BY mp.ordinal",
            )
            .unwrap();
        stmt.query_map([&sid], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(parts, vec![("thinking".into(), 0), ("text".into(), 1)]);
}

#[test]
fn tool_call_file_event_and_git_observation_persist() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blob_store();
    let sid = persist_fixture(&conn, &blobs, "tool_use.jsonl");

    let (name, is_err, out): (String, i64, String) = conn
        .query_row(
            "SELECT name, is_error, output_text FROM tool_call WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(name, "Edit");
    assert_eq!(is_err, 0);
    assert_eq!(out, "File edited successfully");

    let (path, kind): (String, String) = conn
        .query_row(
            "SELECT path, change_kind FROM file_event WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(path, "src/app.ts");
    assert_eq!(kind, "edit");

    let (source, branch): (String, String) = conn
        .query_row(
            "SELECT source, branch FROM git_observation WHERE session_id = ?1 LIMIT 1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(source, "agent_recorded");
    assert_eq!(branch, "fix");
}

#[test]
fn segments_persist_on_cwd_change() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blob_store();
    let sid = persist_fixture(&conn, &blobs, "segments.jsonl");
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM session_segment WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2);
}

#[test]
fn reingest_is_idempotent() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blob_store();
    persist_fixture(&conn, &blobs, "basic_text.jsonl");
    persist_fixture(&conn, &blobs, "basic_text.jsonl");

    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM agent_session", [], |r| r.get(0))
        .unwrap();
    let messages: i64 = conn
        .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sessions, 1, "re-persist must not duplicate the session");
    assert_eq!(messages, 2, "re-persist must not duplicate messages");
}

#[test]
fn out_of_order_parent_resolves() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    // seq 0 references (as parent) the uuid that only appears at seq 1.
    let content = concat!(
        "{\"type\":\"user\",\"uuid\":\"u_a\",\"parentUuid\":\"u_b\",\"sessionId\":\"ooo\",\"message\":{\"role\":\"user\",\"content\":\"child first\"}}\n",
        "{\"type\":\"user\",\"uuid\":\"u_b\",\"parentUuid\":null,\"sessionId\":\"ooo\",\"message\":{\"role\":\"user\",\"content\":\"parent second\"}}\n"
    );
    let parsed = ClaudeCodeAdapter::new().parse_str(content, "fallback");
    let (_bd, blobs) = blob_store();
    let sid = persist_session(&conn, "claude-code", "Claude Code", &parsed, &blobs).unwrap();

    let parent_of_0: Option<String> = conn
        .query_row(
            "SELECT parent_id FROM message WHERE session_id = ?1 AND seq = 0",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    let id_of_1: String = conn
        .query_row(
            "SELECT id FROM message WHERE session_id = ?1 AND seq = 1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        parent_of_0.as_deref(),
        Some(id_of_1.as_str()),
        "forward/out-of-order parent must resolve to the later message"
    );
}

#[test]
fn codex_session_persists_git_and_provider() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blob_store();
    let sid = persist_codex(&conn, &blobs, "minimal.jsonl");

    let (source, branch): (String, String) = conn
        .query_row(
            "SELECT source, branch FROM git_observation WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(source, "agent_recorded");
    assert_eq!(branch, "main");

    let provider: String = conn
        .query_row(
            "SELECT provider FROM session_segment WHERE session_id = ?1 LIMIT 1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(provider, "openai");

    let count: i64 = conn
        .query_row(
            "SELECT message_count FROM agent_session WHERE id = ?1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3, "user + reasoning + assistant");
}

#[test]
fn codex_patch_persists_file_events_and_tool_call() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blob_store();
    let sid = persist_codex(&conn, &blobs, "patch_apply.jsonl");

    let files: i64 = conn
        .query_row(
            "SELECT count(*) FROM file_event WHERE session_id = ?1 AND source = 'agent_patch'",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(files, 3);

    let (path, old): (String, String) = conn
        .query_row(
            "SELECT path, old_path FROM file_event WHERE session_id = ?1 AND change_kind = 'move'",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(path, "src/renamed.ts");
    assert_eq!(old, "src/old.ts");

    let name: String = conn
        .query_row(
            "SELECT name FROM tool_call WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "apply_patch");
}

#[test]
fn codex_cumulative_token_totals_persist() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blob_store();
    let sid = persist_codex(&conn, &blobs, "token_count.jsonl");

    let totals: (i64, i64, i64) = conn
        .query_row(
            "SELECT total_input_tokens, total_output_tokens, total_cache_tokens
             FROM agent_session WHERE id = ?1",
            [&sid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(totals, (120, 30, 45));
}

#[test]
fn recorded_remote_url_is_stored_credential_free() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blob_store();
    // A recorded remote URL that embeds a token must never be persisted verbatim.
    let content = concat!(
        "{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"id\":\"gitcreds\",\"cli_version\":\"1\",\"cwd\":\"/p\",\"git\":{\"branch\":\"main\",\"commit_hash\":\"abc123\",\"repository_url\":\"https://user:ghp_secrettoken@github.com/org/repo.git\"}}}\n",
        "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"hi\"}}\n"
    );
    let parsed = CodexAdapter::new().parse_str(content, "gitcreds");
    let sid = persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();

    let remote: String = conn
        .query_row(
            "SELECT remote_url_norm FROM git_observation WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remote, "github.com/org/repo");
    assert!(
        !remote.contains("ghp_secrettoken"),
        "token must be stripped"
    );
    assert!(!remote.contains('@'));
}

#[test]
fn codex_encrypted_reasoning_persists_opaque_and_non_searchable() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blob_store();
    let content = concat!(
        "{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"id\":\"enc\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}\n",
        "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{\"type\":\"reasoning\",\"summary\":\"plan\",\"encrypted_content\":\"ENCRYPTED-BLOB\"}}\n"
    );
    let parsed = CodexAdapter::new().parse_str(content, "enc");
    let sid = persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();

    // The opaque part is persisted (faithful local storage) but flagged
    // non-searchable, and its cleartext is never surfaced as `text`.
    let (searchable, text): (i64, Option<String>) = conn
        .query_row(
            "SELECT mp.searchable, mp.text FROM message_part mp
             JOIN message m ON m.id = mp.message_id
             WHERE m.session_id = ?1 AND mp.kind = 'opaque'",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(searchable, 0, "encrypted content must never be searchable");
    assert!(
        text.is_none(),
        "encrypted content is never rendered as text"
    );

    // No opaque part ever reaches the FTS-backed search projection.
    let projected: i64 = conn
        .query_row("SELECT count(*) FROM search_document", [], |r| r.get(0))
        .unwrap();
    assert_eq!(projected, 0);
}

#[test]
fn codex_recorded_patch_payloads_persist_as_faithful_blobs() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = blob_store();
    let sid = persist_codex(&conn, &blobs, "patch_apply.jsonl");

    // Every recorded change in the fixture carries a payload, so each file event
    // references a distinct byte-faithful blob.
    let (with_blob, distinct_blobs): (i64, i64) = conn
        .query_row(
            "SELECT count(patch_blob_id), count(DISTINCT patch_blob_id)
             FROM file_event WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        with_blob, 3,
        "all three recorded changes keep their payload"
    );
    assert_eq!(distinct_blobs, 3);

    let read_patch = |path: &str| -> String {
        let (relpath, media, state): (String, String, String) = conn
            .query_row(
                "SELECT b.storage_relpath, b.media_type, b.scan_state
                 FROM file_event f JOIN blob b ON b.id = f.patch_blob_id
                 WHERE f.session_id = ?1 AND f.path = ?2",
                rusqlite::params![sid, path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(media, "text/x-patch");
        // Unscanned canonical storage must not yet be searchable/exportable.
        assert_eq!(state, "pending");
        String::from_utf8(blobs.read(&relpath).unwrap()).unwrap()
    };

    // Update: the exact recorded unified diff round-trips byte-for-byte.
    let edited = read_patch("src/edit.ts");
    assert!(edited.contains("-old") && edited.contains("+new"));
    // Create: the recorded file content round-trips.
    assert!(read_patch("src/new.ts").contains("export const x = 1"));
    // Move: the recorded move diff round-trips.
    assert!(read_patch("src/renamed.ts").contains("moved"));
}
