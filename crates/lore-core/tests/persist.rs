//! M1 acceptance: a parsed Claude session round-trips into SQLite faithfully —
//! ordered mixed content, thinking metadata, tool results, segments on cwd
//! change, out-of-order parent resolution, and idempotent re-ingest.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::ingest::persist_session;
use rusqlite::Connection;

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/claude_code")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn persist_fixture(conn: &Connection, name: &str) -> String {
    let parsed = ClaudeCodeAdapter::new().parse_str(&fixture(name), "fallback");
    persist_session(conn, "claude-code", "Claude Code", &parsed).unwrap()
}

#[test]
fn basic_session_round_trips_in_order() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let sid = persist_fixture(&conn, "basic_text.jsonl");

    let (count, title): (i64, String) = conn
        .query_row(
            "SELECT message_count, title FROM agent_session WHERE id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(title, "Add health check endpoint");

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
    let sid = persist_fixture(&conn, "tool_use.jsonl");

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
    let sid = persist_fixture(&conn, "segments.jsonl");
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
    persist_fixture(&conn, "basic_text.jsonl");
    persist_fixture(&conn, "basic_text.jsonl");

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
    let sid = persist_session(&conn, "claude-code", "Claude Code", &parsed).unwrap();

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
