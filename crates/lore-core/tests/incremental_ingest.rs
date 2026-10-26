//! An append is parsed and persisted from the last ingest's checkpoint. Every
//! row it leaves must equal what a fresh ingest of the same file writes,
//! except for row ids and timestamps.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::codex::CodexAdapter;
use lore_core::adapters::AgentAdapter;
use lore_core::ingest::{ingest_file, IngestOutcome};
use lore_core::storage::blob::BlobStore;
use rusqlite::types::Value;
use rusqlite::Connection;
use serde_json::json;

fn token() -> String {
    format!("ghp{}", "_0123456789abcdefghijklmnopqrstuvwxyz")
}

const TABLES: &[(&str, &str)] = &[
    (
        "agent_session",
        "SELECT id, agent_id, native_session_id, dedupe_key, title, started_at, ended_at,
                primary_model, message_count, tool_call_count, total_input_tokens,
                total_output_tokens, total_cache_tokens, parse_status, parse_note, agent_version
         FROM agent_session WHERE id = ?1",
    ),
    (
        "session_segment",
        "SELECT id, seq_start, seq_end, cwd, model, provider, repository_id
         FROM session_segment WHERE session_id = ?1 ORDER BY seq_start",
    ),
    (
        "message",
        "SELECT id, segment_id, native_uuid, parent_id, parent_native_uuid, seq, role,
                event_kind, is_sidechain, ts, model, token_input, token_output, token_cache,
                stop_reason, source_artifact_id, source_offset, metadata_json
         FROM message WHERE session_id = ?1 ORDER BY seq",
    ),
    (
        "message_part",
        "SELECT p.id, p.message_id, p.ordinal, p.kind, p.text, p.content_json, p.searchable,
                p.metadata_json
         FROM message_part p JOIN message m ON m.id = p.message_id
         WHERE m.session_id = ?1 ORDER BY m.seq, p.ordinal",
    ),
    (
        "tool_call",
        "SELECT id, call_part_id, result_part_id, native_call_id, name, input_json,
                output_text, is_error, duration_ms
         FROM tool_call WHERE session_id = ?1 ORDER BY id",
    ),
    (
        "file_event",
        "SELECT f.id, f.segment_id, f.tool_call_id, f.path, f.change_kind, f.old_path,
                f.lines_added, f.lines_removed, f.patch_blob_id, b.scan_state, f.source,
                f.event_ts, f.content_oid
         FROM file_event f LEFT JOIN blob b ON b.id = f.patch_blob_id
         WHERE f.session_id = ?1 ORDER BY f.id",
    ),
    (
        "secret_finding",
        "SELECT id, source_kind, source_id, field, rule, span_start, span_end, severity,
                value_fingerprint, disposition
         FROM secret_finding WHERE session_id = ?1 ORDER BY id",
    ),
    (
        "search_document",
        "SELECT segment_id, source_kind, source_id, field, ordinal, redacted_text, started_at,
                agent_id
         FROM search_document WHERE session_id = ?1 ORDER BY source_kind, source_id, field",
    ),
    (
        "git_observation",
        "SELECT id, segment_id, source, branch, commit_sha, remote_url_norm
         FROM git_observation WHERE session_id = ?1 ORDER BY id",
    ),
    (
        "search_git",
        "SELECT segment_id, repository_id, source_class, branch, commit_sha
         FROM search_git WHERE session_id = ?1 ORDER BY segment_id, source_class, branch",
    ),
    (
        "session_source",
        "SELECT source_artifact_id, first_seq, last_seq FROM session_source
         WHERE session_id = ?1",
    ),
    (
        "source",
        "SELECT sa.size, sa.full_hash, sa.prefix_hash, sa.generation, i.last_offset,
                i.last_line, i.prefix_hash, i.parser_version, i.state, i.resume_json
         FROM session_source ss
         JOIN source_artifact sa ON sa.id = ss.source_artifact_id
         JOIN ingest_state i ON i.source_artifact_id = sa.id
         WHERE ss.session_id = ?1",
    ),
];

type Dump = BTreeMap<&'static str, Vec<Vec<Value>>>;

fn dump(conn: &Connection, session_id: &str) -> Dump {
    TABLES
        .iter()
        .map(|(name, sql)| {
            let mut stmt = conn.prepare(sql).unwrap();
            let width = stmt.column_count();
            let rows = stmt
                .query_map([session_id], |r| (0..width).map(|i| r.get(i)).collect())
                .unwrap()
                .collect::<rusqlite::Result<Vec<Vec<Value>>>>()
                .unwrap();
            (*name, rows)
        })
        .collect()
}

fn message_rowids(conn: &Connection, session_id: &str) -> Vec<(i64, String)> {
    let mut stmt = conn
        .prepare("SELECT rowid, id FROM message WHERE session_id = ?1 ORDER BY seq")
        .unwrap();
    let rows = stmt
        .query_map([session_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows
}

fn session_id(outcome: IngestOutcome) -> String {
    match outcome {
        IngestOutcome::Ingested { session_id, .. } => Some(session_id),
        IngestOutcome::Skipped => None,
    }
    .expect("a changed file must be ingested")
}

/// Grow a file through `cuts` (byte offsets into `content`), ingesting each
/// prefix into one archive and, separately, into a fresh one. Every step must
/// leave the same rows. Returns, per step after the first, whether the ingest
/// kept the session row (an append) rather than replacing the session, and the
/// grown archive with the session's id.
fn grow_and_compare(
    adapter: &dyn AgentAdapter,
    content: &str,
    cuts: &[usize],
) -> (Vec<bool>, Connection, String) {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let path = dir.path().join("session.jsonl");
    let live = lore_core::storage::open_in_memory().unwrap();
    live.execute_batch(
        "CREATE TEMP TABLE replaced (session_id TEXT);
         CREATE TEMP TRIGGER log_replace AFTER DELETE ON agent_session
         BEGIN INSERT INTO replaced VALUES (old.id); END;",
    )
    .unwrap();

    let mut kept = Vec::new();
    let mut session = String::new();
    for (step, &cut) in cuts.iter().enumerate() {
        std::fs::write(&path, &content[..cut]).unwrap();
        live.execute_batch("DELETE FROM replaced").unwrap();
        let id = session_id(ingest_file(&live, adapter, &path, &blobs).unwrap());
        if step > 0 {
            let replaced: i64 = live
                .query_row("SELECT count(*) FROM replaced", [], |r| r.get(0))
                .unwrap();
            kept.push(replaced == 0);
        }
        // `rank = 1` also checks the index against search_document.
        live.execute_batch(
            "INSERT INTO search_fts(search_fts, rank) VALUES('integrity-check', 1);",
        )
        .unwrap();

        let fresh = lore_core::storage::open_in_memory().unwrap();
        let fresh_id = session_id(ingest_file(&fresh, adapter, &path, &blobs).unwrap());
        assert_eq!(fresh_id, id);
        assert_eq!(
            dump(&live, &id),
            dump(&fresh, &id),
            "rows after step {step} (cut at byte {cut}) differ from a fresh ingest"
        );
        session = id;
    }
    (kept, live, session)
}

/// (title, message_count, tool_call_count, input, output, cache, parse_note)
type Summary = (
    String,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

fn summary(conn: &Connection, session_id: &str) -> Summary {
    conn.query_row(
        "SELECT title, message_count, tool_call_count, total_input_tokens,
                total_output_tokens, total_cache_tokens, parse_note
         FROM agent_session WHERE id = ?1",
        [session_id],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        },
    )
    .unwrap()
}

fn jsonl(lines: &[serde_json::Value]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

fn line_ends(content: &str) -> Vec<usize> {
    content.match_indices('\n').map(|(i, _)| i + 1).collect()
}

fn claude_session() -> String {
    let sid = "cc-eq";
    let ctx =
        |cwd: &str| json!({"sessionId": sid, "cwd": cwd, "gitBranch": "main", "version": "1.2.3"});
    let with = |base: serde_json::Value, extra: serde_json::Value| {
        let mut v = base;
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    };
    jsonl(&[
        with(
            ctx("/repo/app"),
            json!({"type": "user", "uuid": "u1", "parentUuid": null,
            "timestamp": "2026-08-10T10:00:00.000Z",
            "message": {"role": "user", "content": "Fix the flaky cache test"}}),
        ),
        with(
            ctx("/repo/app"),
            json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1",
            "timestamp": "2026-08-10T10:00:05.000Z",
            "message": {"role": "assistant", "model": "claude-x", "stop_reason": "tool_use",
                "usage": {"input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 50},
                "content": [
                    {"type": "thinking", "thinking": "look at the cache", "signature": "sig"},
                    {"type": "text", "text": "Editing the cache."},
                    {"type": "tool_use", "id": "t1", "name": "Edit",
                     "input": {"file_path": "/repo/app/src/cache.rs", "old_string": "a", "new_string": "b"}}
                ]}}),
        ),
        json!({"type": "file-history-snapshot", "messageId": "a1"}),
        with(
            ctx("/repo/app"),
            json!({"type": "user", "uuid": "u2", "parentUuid": "a1",
            "timestamp": "2026-08-10T10:00:06.000Z",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "edited", "is_error": false}
            ]}}),
        ),
        with(
            ctx("/repo/app"),
            json!({"type": "assistant", "uuid": "a2", "parentUuid": "u2",
            "timestamp": "2026-08-10T10:00:07.000Z",
            "message": {"role": "assistant", "model": "claude-x",
                "usage": {"input_tokens": 7, "output_tokens": 3},
                "content": [{"type": "tool_use", "id": "t2", "name": "Write",
                    "input": {"file_path": "/repo/app/NOTES.md", "content": format!("key {}", token())}}]}}),
        ),
        json!({"type": "ai-title", "aiTitle": "Cache test fix", "sessionId": sid}),
        with(
            ctx("/repo/lib"),
            json!({"type": "user", "uuid": "u3", "parentUuid": "a2",
            "timestamp": "2026-08-10T10:00:08.000Z",
            "message": {"role": "user", "content": format!("deploy with {}", token())}}),
        ),
        with(
            ctx("/repo/lib"),
            json!({"type": "assistant", "uuid": "a3", "parentUuid": "u3",
            "isSidechain": true, "timestamp": "2026-08-10T10:00:09.000Z",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "sidechain note"}]}}),
        ),
        json!({"type": "custom-title", "customTitle": "Renamed", "sessionId": sid}),
        with(
            ctx("/repo/lib"),
            json!({"type": "user", "uuid": "u4", "parentUuid": "u9",
            "timestamp": "2026-08-10T10:00:10.000Z",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t-missing", "content": "lost"}
            ]}}),
        ),
        with(
            ctx("/repo/lib"),
            json!({"type": "assistant", "uuid": "u9", "parentUuid": "a3",
            "timestamp": "2026-08-10T10:00:11.000Z",
            "message": {"role": "assistant", "content": "late parent"}}),
        ),
        with(
            ctx("/repo/app"),
            json!({"type": "user", "uuid": "u5", "parentUuid": "u9",
            "timestamp": "2026-08-10T10:00:12.000Z",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t2", "content": [{"type": "text", "text": "written"}]}
            ]}}),
        ),
    ])
}

#[test]
fn claude_code_appends_match_a_fresh_ingest_line_by_line() {
    let content = claude_session();
    let (kept, live, id) =
        grow_and_compare(&ClaudeCodeAdapter::new(), &content, &line_ends(&content));
    // Line 11 is the message an earlier line named as its parent: the earlier
    // row changes, so that one append replaces the session.
    assert_eq!(
        kept,
        [true, true, true, true, true, true, true, true, true, false, true]
    );
    assert_eq!(summary(&live, &id), claude_summary());
}

fn claude_summary() -> Summary {
    (
        "Renamed".to_string(),
        9,
        2,
        Some(107),
        Some(23),
        Some(50),
        Some("2 parser note(s); first: unhandled event type: file-history-snapshot".to_string()),
    )
}

#[test]
fn claude_code_appends_match_a_fresh_ingest_in_chunks() {
    let content = claude_session();
    let ends = line_ends(&content);
    let cuts = [ends[1], ends[4], ends[8], ends[11]];
    let (kept, live, id) = grow_and_compare(&ClaudeCodeAdapter::new(), &content, &cuts);
    // The late parent arrives in the same append as the message naming it.
    assert_eq!(kept, [true, true, true]);
    assert_eq!(summary(&live, &id), claude_summary());
}

fn codex_session() -> String {
    let at = |s: u32| format!("2026-08-11T10:00:{s:02}.000Z");
    let item = |s: u32, payload: serde_json::Value| json!({"type": "response_item", "timestamp": at(s), "payload": payload});
    let event = |s: u32, payload: serde_json::Value| json!({"type": "event_msg", "timestamp": at(s), "payload": payload});
    jsonl(&[
        json!({"type": "session_meta", "timestamp": at(0), "payload": {
            "id": "cx-eq", "cli_version": "0.9", "cwd": "/repo", "model_provider": "openai",
            "git": {"branch": "main", "commit_hash": "abc123",
                    "repository_url": "https://github.com/o/r.git"}}}),
        json!({"type": "turn_context", "timestamp": at(1),
               "payload": {"cwd": "/repo", "model": "gpt-5"}}),
        item(
            2,
            json!({"type": "message", "role": "user",
            "content": [{"type": "input_text", "text": "Add a config loader"}]}),
        ),
        item(
            3,
            json!({"type": "reasoning", "summary": [{"type": "summary_text", "text": "plan"}],
            "encrypted_content": "opaque"}),
        ),
        item(
            4,
            json!({"type": "function_call", "name": "apply_patch", "arguments": "{}",
            "call_id": "c1"}),
        ),
        event(
            5,
            json!({"type": "token_count", "info": {"total_token_usage":
            {"input_tokens": 10, "output_tokens": 5, "cached_input_tokens": 2}}}),
        ),
        item(
            6,
            json!({"type": "function_call_output", "call_id": "c1", "output": "Done"}),
        ),
        event(
            7,
            json!({"type": "patch_apply_end", "call_id": "c1", "success": true,
            "changes": {"src/config.ts": {"type": "add",
                "content": format!("const KEY = '{}'", token())}}}),
        ),
        json!({"type": "turn_context", "timestamp": at(8),
               "payload": {"cwd": "/repo/pkg", "model": "gpt-5"}}),
        item(
            9,
            json!({"type": "message", "role": "assistant",
            "content": [{"type": "output_text", "text": "Loader added"}]}),
        ),
        event(10, json!({"type": "brand_new_event"})),
        json!({"type": "compacted", "timestamp": at(11), "payload": {"message": "summary"}}),
        event(
            12,
            json!({"type": "patch_apply_end", "call_id": "c2", "success": true,
            "changes": {"a.ts": {"type": "update", "unified_diff": "@@ -1 +1 @@\n-x\n+y\n"}}}),
        ),
        item(
            13,
            json!({"type": "function_call", "name": "shell", "arguments": "{}",
            "call_id": "c2"}),
        ),
        event(
            14,
            json!({"type": "token_count", "info": {"total_token_usage":
            {"input_tokens": 40, "output_tokens": 9}}}),
        ),
        json!({"type": "session_meta", "timestamp": at(15), "payload": {
            "id": "cx-eq", "cli_version": "0.9", "cwd": "/repo", "model_provider": "azure"}}),
    ])
}

#[test]
fn codex_appends_match_a_fresh_ingest_line_by_line() {
    let content = codex_session();
    let (kept, live, id) = grow_and_compare(&CodexAdapter::new(), &content, &line_ends(&content));
    // The session has no message, and so no checkpoint, until line 3. Line 14
    // carries the call an earlier patch named, and line 16 restates the
    // session meta that every segment carries: both replace the session.
    assert_eq!(
        kept,
        [
            false, false, true, true, true, true, true, true, true, true, true, true, false, true,
            false
        ]
    );
    assert_eq!(summary(&live, &id), codex_summary());
}

fn codex_summary() -> Summary {
    (
        "Add a config loader".to_string(),
        7,
        2,
        Some(40),
        Some(9),
        None,
        Some("1 parser note(s); first: unknown event_msg: brand_new_event".to_string()),
    )
}

#[test]
fn codex_appends_match_a_fresh_ingest_in_chunks() {
    let content = codex_session();
    let ends = line_ends(&content);
    let cuts = [ends[3], ends[5], ends[9], ends[12], ends[14]];
    let (kept, live, id) = grow_and_compare(&CodexAdapter::new(), &content, &cuts);
    assert_eq!(kept, [true, true, true, false]);
    assert_eq!(summary(&live, &id), codex_summary());
}

// A watcher can fire while the agent is mid-way through writing a line. The
// half line parses as a partial session, and once the line is complete the
// session is whole again, exactly as a fresh ingest of each state reads.
#[test]
fn a_line_split_across_appends_matches_a_fresh_ingest() {
    let content = claude_session();
    let ends = line_ends(&content);
    let mid = |i: usize| ends[i - 1] + (ends[i] - ends[i - 1]) / 2;
    let cuts = [
        ends[0],
        mid(1),
        ends[1],
        mid(3),
        mid(3) + 7,
        ends[3],
        ends[4],
    ];
    let (kept, _, _) = grow_and_compare(&ClaudeCodeAdapter::new(), &content, &cuts);
    // A half line leaves no checkpoint, so the step after it parses the whole
    // file again.
    assert_eq!(kept, [true, false, true, false, false, true]);
}

#[test]
fn a_split_line_reads_partial_then_whole() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let path = dir.path().join("session.jsonl");
    let conn = lore_core::storage::open_in_memory().unwrap();
    let adapter = ClaudeCodeAdapter::new();
    let content = claude_session();
    let ends = line_ends(&content);

    let status = |conn: &Connection| -> (String, Option<String>, i64) {
        conn.query_row(
            "SELECT parse_status, parse_note, message_count FROM agent_session",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };

    std::fs::write(&path, &content[..ends[0]]).unwrap();
    ingest_file(&conn, &adapter, &path, &blobs).unwrap();
    std::fs::write(&path, &content[..ends[0] + 40]).unwrap();
    ingest_file(&conn, &adapter, &path, &blobs).unwrap();
    assert_eq!(
        status(&conn),
        (
            "partial".to_string(),
            Some("1 parser note(s); first: unparseable line (possibly truncated)".to_string()),
            1
        )
    );

    std::fs::write(&path, &content[..ends[1]]).unwrap();
    ingest_file(&conn, &adapter, &path, &blobs).unwrap();
    assert_eq!(status(&conn), ("ok".to_string(), None, 2));
}

#[test]
fn a_secret_in_an_appended_line_is_redacted_and_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let path = dir.path().join("session.jsonl");
    let conn = lore_core::storage::open_in_memory().unwrap();
    let adapter = ClaudeCodeAdapter::new();
    let content = claude_session();
    let ends = line_ends(&content);

    std::fs::write(&path, &content[..ends[5]]).unwrap();
    let sid = session_id(ingest_file(&conn, &adapter, &path, &blobs).unwrap());
    let before = message_rowids(&conn, &sid);
    std::fs::write(&path, &content[..ends[6]]).unwrap();
    ingest_file(&conn, &adapter, &path, &blobs).unwrap();
    assert_eq!(&message_rowids(&conn, &sid)[..before.len()], &before[..]);

    let (text, rule, span): (String, String, (i64, i64)) = conn
        .query_row(
            "SELECT d.redacted_text, f.rule, f.span_start, f.span_end
             FROM search_document d
             JOIN message_part p ON p.id = d.source_id
             JOIN message m ON m.id = p.message_id
             JOIN secret_finding f ON f.source_id = p.id AND f.field = 'text'
             WHERE m.seq = 4",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, (r.get(2)?, r.get(3)?))),
        )
        .unwrap();
    assert_eq!(text, "deploy with «redacted:github-token»");
    assert_eq!(rule, "github-token");
    assert_eq!(span, (12, 52));

    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM search_fts WHERE search_fts MATCH 'deploy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 1, "the appended line is searchable");
}
