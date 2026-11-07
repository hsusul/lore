//! Tool output is stored once (migration 0020, `lore_core::compact`): a tool
//! result's payload drops the output it repeats from `text` when it holds it
//! exactly once, new ingests write that slim form, older rows are converted by
//! a verified, resumable migration, and every reader still gets the exact
//! bytes that were ingested.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::AtomicBool;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::codex::CodexAdapter;
use lore_core::compact::{self, SlimReport};
use lore_core::export::export_session_markdown;
use lore_core::forget::forget_session;
use lore_core::ingest::persist_session;
use lore_core::model::ParsedSession;
use lore_core::search::search;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;
use serde_json::{json, Value};

/// A string output with every escape JSON has.
const STRING_OUT: &str = "listing frobnicate\n\t\"quoted\" back\\slash \u{1} é 🦀 done";
const BLOCK_OUT: &str = "one block of frobnicate output, long enough to slim";
const CODEX_OUT: &str = "README.md\nsrc/frobnicate.rs\nCargo.toml\n";

fn store() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    (dir, s)
}

fn lines(values: &[Value]) -> String {
    values.iter().map(|v| format!("{v}\n")).collect()
}

/// A Claude Code session with one tool call per result shape: a string, one
/// text block, and two text blocks (whose joined text the payload never holds).
fn claude(session: &str) -> ParsedSession {
    let results = [
        json!(STRING_OUT),
        json!([{ "type": "text", "text": BLOCK_OUT }]),
        json!([{ "type": "text", "text": "first frobnicate block" },
               { "type": "text", "text": "second block" }]),
    ];
    let mut events = vec![json!({"type": "user", "uuid": format!("u0{session}"),
        "sessionId": session, "cwd": "/p", "timestamp": "2026-08-11T10:00:00.000Z",
        "message": {"role": "user", "content": "list the frobnicate files"}})];
    for (i, content) in results.iter().enumerate() {
        events.push(
            json!({"type": "assistant", "uuid": format!("a{i}{session}"),
            "sessionId": session, "cwd": "/p", "timestamp": format!("2026-08-11T10:00:0{i}.100Z"),
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": format!("t{i}"), "name": "Bash",
                 "input": {"command": "ls"}}]}}),
        );
        events.push(json!({"type": "user", "uuid": format!("r{i}{session}"),
            "sessionId": session, "cwd": "/p", "timestamp": format!("2026-08-11T10:00:0{i}.200Z"),
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": format!("t{i}"), "content": content}]}}));
    }
    ClaudeCodeAdapter::new().parse_str(&lines(&events), session)
}

/// A Codex session with a string output, an array output (stored raw), and a
/// short output (too short to be worth a marker).
fn codex() -> ParsedSession {
    let outputs = [
        json!(CODEX_OUT),
        json!([{"type": "input_text", "text": "Script completed\n"},
               {"type": "input_text", "text": "{\"frobnicate\":true}"}]),
        json!("ok"),
    ];
    let mut events = vec![
        json!({"type": "session_meta", "timestamp": "2026-10-01T10:00:00.000Z",
        "payload": {"id": "019e0000-0000-7000-8000-000000000002", "cwd": "/proj",
                    "cli_version": "0.133.0", "model_provider": "openai"}}),
    ];
    for (i, output) in outputs.iter().enumerate() {
        events.push(
            json!({"type": "response_item", "timestamp": format!("2026-10-01T10:00:0{i}.100Z"),
            "payload": {"type": "function_call", "name": "shell", "call_id": format!("call_{i}"),
                        "arguments": "{\"command\":[\"ls\"]}"}}),
        );
        events.push(
            json!({"type": "response_item", "timestamp": format!("2026-10-01T10:00:0{i}.200Z"),
            "payload": {"type": "function_call_output", "call_id": format!("call_{i}"),
                        "output": output}}),
        );
    }
    CodexAdapter::new().parse_str(&lines(&events), "codex-k")
}

/// Ingest two Claude Code sessions and one Codex session.
fn three_sessions(conn: &Connection, blobs: &BlobStore) -> Vec<String> {
    vec![
        persist_session(conn, "claude-code", "Claude Code", &claude("sa"), blobs).unwrap(),
        persist_session(conn, "claude-code", "Claude Code", &claude("sb"), blobs).unwrap(),
        persist_session(conn, "codex", "Codex", &codex(), blobs).unwrap(),
    ]
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Every tool result payload as the one read path serves it, in row order.
fn served(conn: &Connection) -> Vec<(String, Option<String>)> {
    conn.prepare(
        "SELECT v.id, v.content_json FROM message_part_full v JOIN message_part p ON p.id = v.id
         WHERE v.kind = 'tool_result' ORDER BY p.rowid",
    )
    .unwrap()
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn slim_rows(conn: &Connection) -> i64 {
    count(
        conn,
        "SELECT count(*) FROM message_part
         WHERE kind = 'tool_result' AND instr(content_json, char(1)) > 0",
    )
}

/// The payloads the adapters parsed, in ingest order.
fn parsed_payloads() -> Vec<String> {
    [claude("sa"), claude("sb"), codex()]
        .iter()
        .flat_map(|s| s.messages.iter().flat_map(|m| m.parts.iter()))
        .filter(|p| p.kind.as_str() == "tool_result")
        .map(|p| p.content_json.clone().unwrap())
        .collect()
}

/// Put tool results back in the shape an archive ingested before migration
/// 0020 has: every payload whole, every row up to now still to examine.
fn make_legacy(conn: &Connection) {
    conn.execute_batch(
        "UPDATE message_part
            SET content_json = (SELECT v.content_json FROM message_part_full v
                                 WHERE v.id = message_part.id)
          WHERE kind = 'tool_result' AND instr(content_json, char(1)) > 0;
         INSERT OR REPLACE INTO setting (key, value_json, updated_at)
            SELECT 'compact.tool_output_until', CAST(max(rowid) AS TEXT), 0 FROM message_part;
         DELETE FROM setting WHERE key IN ('compact.tool_output_after',
                                           'compact.tool_output_failed_rows');",
    )
    .unwrap();
}

/// What a reader sees: each session's detail and Markdown export (masked and
/// full) and the hits of a few searches.
fn reader_view(conn: &Connection, sessions: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for s in sessions {
        out.push(format!(
            "{:?}",
            lore_core::query::get_session(conn, s).unwrap()
        ));
        out.push(export_session_markdown(conn, s, false).unwrap().unwrap());
        out.push(export_session_markdown(conn, s, true).unwrap().unwrap());
    }
    for q in ["frobnicate", "README", "quoted", "Script completed"] {
        out.push(format!("{:?}", search(conn, q, 50).unwrap()));
    }
    out
}

#[test]
fn ingest_stores_the_slim_form_and_reads_back_byte_equal() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    three_sessions(&conn, &blobs);

    let payloads: Vec<String> = served(&conn).into_iter().map(|(_, c)| c.unwrap()).collect();
    assert_eq!(payloads, parsed_payloads(), "byte-equal to what was parsed");

    // Claude: string and one-block shapes slim (quoted), two blocks do not.
    // Codex: string output (quoted) and array output (raw) slim, "ok" not.
    assert_eq!(slim_rows(&conn), 2 + 2 + 2);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message_part WHERE instr(content_json, char(1) || 'lore:text:raw') > 0"
        ),
        1
    );
    // Nothing to convert later: rows ingested now are past the watermark.
    assert!(!compact::tool_outputs_pending(&conn).unwrap());
}

#[test]
fn the_migration_converts_each_shape_and_reads_back_byte_equal() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sessions = three_sessions(&conn, &blobs);
    let view_slim = reader_view(&conn, &sessions);
    make_legacy(&conn);
    assert_eq!(slim_rows(&conn), 0);
    let before = served(&conn);
    assert_eq!(
        reader_view(&conn, &sessions),
        view_slim,
        "same view either way"
    );
    assert!(compact::tool_outputs_pending(&conn).unwrap());

    // A batch of one exercises the resume path between every row.
    let report = compact::slim_tool_outputs(&conn, 1, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        (report.converted, report.kept, report.failed, report.pending),
        (6, 3, 0, false)
    );
    assert!(report.bytes_saved > 0);
    assert_eq!(slim_rows(&conn), 6);
    assert_eq!(served(&conn), before, "payloads read back byte-equal");
    assert_eq!(
        reader_view(&conn, &sessions),
        view_slim,
        "export and search unchanged"
    );

    // Rerunning is a no-op.
    let again = compact::slim_tool_outputs(&conn, 1, &AtomicBool::new(false)).unwrap();
    assert_eq!(again, SlimReport::default());
    assert_eq!(served(&conn), before);
}

#[test]
fn a_payload_that_does_not_hold_the_output_exactly_once_is_left_alone() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    three_sessions(&conn, &blobs);
    make_legacy(&conn);
    // The output twice, once mangled, and a payload that is not UTF-8.
    let ids: Vec<String> = served(&conn)
        .into_iter()
        .map(|(id, _)| id)
        .take(3)
        .collect();
    conn.execute(
        "UPDATE message_part SET content_json = json_array(text, text) WHERE id = ?1",
        [&ids[0]],
    )
    .unwrap();
    conn.execute(
        "UPDATE message_part SET content_json = json_quote(text || 'x') WHERE id = ?1",
        [&ids[1]],
    )
    .unwrap();
    conn.execute(
        "UPDATE message_part SET content_json = CAST(x'22ff22' AS TEXT) WHERE id = ?1",
        [&ids[2]],
    )
    .unwrap();
    let before: Vec<(String, Vec<u8>)> = conn
        .prepare("SELECT id, CAST(content_json AS BLOB) FROM message_part WHERE id IN (?1, ?2, ?3)")
        .unwrap()
        .query_map([&ids[0], &ids[1], &ids[2]], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    let report = compact::slim_tool_outputs(&conn, 100, &AtomicBool::new(false)).unwrap();
    assert_eq!((report.converted, report.kept, report.failed), (4, 5, 0));
    let after: Vec<(String, Vec<u8>)> = conn
        .prepare("SELECT id, CAST(content_json AS BLOB) FROM message_part WHERE id IN (?1, ?2, ?3)")
        .unwrap()
        .query_map([&ids[0], &ids[1], &ids[2]], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(after, before, "both columns kept as they were");
}

#[test]
fn a_mismatch_is_rolled_back_counted_and_skipped() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    three_sessions(&conn, &blobs);
    make_legacy(&conn);
    let before = served(&conn);
    let victim = before[0].0.clone();
    // Damage between the rewrite and the read-back: the victim's text changes,
    // so its rebuilt payload no longer matches.
    conn.execute_batch(&format!(
        "CREATE TRIGGER damage AFTER UPDATE OF content_json ON message_part
         WHEN NEW.id = '{victim}'
         BEGIN UPDATE message_part SET text = text || '!' WHERE id = NEW.id; END;"
    ))
    .unwrap();

    let report = compact::slim_tool_outputs(&conn, 100, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        (report.converted, report.failed),
        (5, 1),
        "the rest carried on"
    );
    conn.execute_batch("DROP TRIGGER damage").unwrap();
    assert_eq!(served(&conn), before, "no reader sees a changed byte");
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message_part WHERE instr(text, '!') > 0"
        ),
        0,
        "the damage was rolled back with the row"
    );
    assert_eq!(
        lore_core::settings::get(&conn, compact::TOOL_OUTPUT_FAILED_SETTING)
            .unwrap()
            .as_deref(),
        Some("1")
    );
}

#[test]
fn an_error_mid_batch_leaves_every_row_intact_and_the_rerun_finishes() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sessions = three_sessions(&conn, &blobs);
    make_legacy(&conn);
    let before = served(&conn);
    let raw_before: Vec<Option<String>> = conn
        .prepare("SELECT content_json FROM message_part WHERE kind = 'tool_result' ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    // The fourth row's rewrite fails: rows one to three were already rewritten
    // in the same batch.
    let victim = before[3].0.clone();
    conn.execute_batch(&format!(
        "CREATE TRIGGER boom BEFORE UPDATE OF content_json ON message_part
         WHEN NEW.id = '{victim}' BEGIN SELECT RAISE(ABORT, 'disk on fire'); END;"
    ))
    .unwrap();

    assert!(compact::slim_tool_outputs(&conn, 100, &AtomicBool::new(false)).is_err());
    let raw_after: Vec<Option<String>> = conn
        .prepare("SELECT content_json FROM message_part WHERE kind = 'tool_result' ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(raw_after, raw_before, "the whole batch rolled back");
    assert!(
        compact::tool_outputs_pending(&conn).unwrap(),
        "cursor not advanced"
    );

    conn.execute_batch("DROP TRIGGER boom").unwrap();
    let report = compact::slim_tool_outputs(&conn, 100, &AtomicBool::new(false)).unwrap();
    assert_eq!((report.converted, report.pending), (6, false));
    assert_eq!(served(&conn), before);
    let _ = sessions;
}

#[test]
fn a_stop_request_leaves_the_rest_for_the_next_run() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    three_sessions(&conn, &blobs);
    make_legacy(&conn);
    let before = served(&conn);
    let stopped = compact::slim_tool_outputs(&conn, 1, &AtomicBool::new(true)).unwrap();
    assert_eq!((stopped.converted, stopped.pending), (0, true));
    let resumed = compact::slim_tool_outputs(&conn, 1, &AtomicBool::new(false)).unwrap();
    assert_eq!((resumed.converted, resumed.pending), (6, false));
    assert_eq!(served(&conn), before);
}

#[test]
fn compact_runs_the_conversion() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    three_sessions(&conn, &blobs);
    make_legacy(&conn);
    let before = served(&conn);
    let report = compact::compact(&conn, &AtomicBool::new(false)).unwrap();
    assert_eq!((report.slim.converted, report.slim.pending), (6, false));
    assert_eq!(served(&conn), before);
}

#[test]
fn compact_backs_up_first_then_returns_the_freed_pages() {
    let dir = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open(&dir.path().join("lore.db")).unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    three_sessions(&conn, &blobs);
    make_legacy(&conn);
    // One large output, so the conversion frees whole pages.
    let big = served(&conn)[0].0.clone();
    conn.execute(
        "UPDATE message_part SET text = printf('%.*c', 2000000, 'x'),
                                 content_json = json_quote(printf('%.*c', 2000000, 'x'))
          WHERE id = ?1",
        [&big],
    )
    .unwrap();
    let before = served(&conn);
    let backups =
        || lore_core::backup::list_backups(&dir.path().join(lore_core::paths::BACKUPS_DIRNAME));

    let report = compact::compact(&conn, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.slim.converted, 6);
    assert_eq!(report.vacuum, compact::VacuumOutcome::AlreadyIncremental);
    assert!(report.reclaimed_pages > 400, "{}", report.reclaimed_pages);
    assert_eq!(compact::freelist_count(&conn).unwrap(), 0);
    assert_eq!(backups().unwrap().len(), 1, "backed up before converting");
    assert_eq!(served(&conn), before);

    let again = compact::compact(&conn, &AtomicBool::new(false)).unwrap();
    assert_eq!(again.slim, SlimReport::default());
    assert_eq!(backups().unwrap().len(), 1, "nothing left to back up for");
}

#[test]
fn forget_is_unaffected() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sessions = three_sessions(&conn, &blobs);
    let kept = reader_view(&conn, &sessions[1..]);
    forget_session(&conn, &blobs, &sessions[0]).unwrap();
    assert_eq!(
        count(
            &conn,
            &format!(
                "SELECT count(*) FROM message_part p JOIN message m ON m.id = p.message_id
                 WHERE m.session_id = '{}'",
                sessions[0]
            )
        ),
        0
    );
    let after = reader_view(&conn, &sessions[1..]);
    // The searches no longer find the forgotten session; the sessions kept
    // read back the same.
    assert_eq!(after[..6], kept[..6]);
    assert!(search(&conn, "frobnicate", 50)
        .unwrap()
        .iter()
        .all(|h| h.session_id != sessions[0]));
}
