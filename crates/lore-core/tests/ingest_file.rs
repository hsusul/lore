//! M1 source-artifact lifecycle acceptance: checkpoints and normalized rows
//! advance atomically across append, rewrite, truncate, move, and restart.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::codex::CodexAdapter;
use lore_core::ingest::{ingest_file, ChangeClass, IngestOutcome};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

const USER_LINE: &str = concat!(
    r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"lifecycle","cwd":"/repo/demo","gitBranch":"main","message":{"role":"user","content":"first"}}"#,
    "\n"
);
const ASSISTANT_LINE: &str = concat!(
    r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"lifecycle","cwd":"/repo/demo","gitBranch":"main","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#,
    "\n"
);

/// A process-wide blob store in a temp dir kept alive for the test binary. The
/// Claude fixtures used here carry no recorded patch payloads, so no blob files
/// are written; the store only satisfies the ingest signature.
fn blobs() -> &'static BlobStore {
    use std::sync::OnceLock;
    static STORE: OnceLock<(tempfile::TempDir, BlobStore)> = OnceLock::new();
    &STORE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let store = BlobStore::open(dir.path()).unwrap();
            (dir, store)
        })
        .1
}

fn ingest(conn: &Connection, path: &Path) -> IngestOutcome {
    ingest_file(conn, &ClaudeCodeAdapter::new(), path, blobs()).unwrap()
}

fn generation(conn: &Connection) -> i64 {
    conn.query_row("SELECT generation FROM source_artifact", [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn message_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT message_count FROM agent_session", [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn codex_patch_without_persisted_tool_call_keeps_a_null_link() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-orphan-patch.jsonl");
    let conn = lore_core::storage::open_in_memory().unwrap();
    let content = concat!(
        r#"{"type":"session_meta","payload":{"id":"orphan-patch","cwd":"/repo/demo"}}"#,
        "\n",
        r#"{"type":"event_msg","payload":{"type":"patch_apply_end","call_id":"missing-call","changes":{"src/lib.rs":{"type":"update","unified_diff":"@@ -1 +1 @@\n-old\n+new\n"}}}}"#,
        "\n"
    );
    fs::write(&path, content).unwrap();

    let outcome = ingest_file(&conn, &CodexAdapter::new(), &path, blobs()).unwrap();
    assert!(matches!(outcome, IngestOutcome::Ingested { .. }));

    let (events, linked): (i64, i64) = conn
        .query_row(
            "SELECT count(*), count(tool_call_id) FROM file_event",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(events, 1, "the recorded patch must be preserved");
    assert_eq!(linked, 0, "a missing optional tool call stays unlinked");
}

#[test]
fn append_rewrite_and_truncate_replace_rows_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let conn = lore_core::storage::open_in_memory().unwrap();

    fs::write(&path, USER_LINE).unwrap();
    let IngestOutcome::Ingested {
        generation: first_generation,
        change: ChangeClass::New,
        ..
    } = ingest(&conn, &path)
    else {
        panic!("first pass must ingest a new source");
    };
    assert_eq!(first_generation, 0);
    assert_eq!(message_count(&conn), 1);

    assert_eq!(ingest(&conn, &path), IngestOutcome::Skipped);

    fs::write(&path, format!("{USER_LINE}{ASSISTANT_LINE}")).unwrap();
    let IngestOutcome::Ingested {
        generation: append_generation,
        change: ChangeClass::Appended,
        ..
    } = ingest(&conn, &path)
    else {
        panic!("a small-file append must be classified as append");
    };
    assert_eq!(append_generation, 0, "append keeps the source generation");
    assert_eq!(message_count(&conn), 2);

    let rewritten = format!("{}{ASSISTANT_LINE}", USER_LINE.replace("first", "other"));
    fs::write(&path, rewritten).unwrap();
    let IngestOutcome::Ingested {
        generation: rewrite_generation,
        change: ChangeClass::Rewritten,
        ..
    } = ingest(&conn, &path)
    else {
        panic!("same-size changed content must be classified as rewrite");
    };
    assert_eq!(rewrite_generation, 1);
    assert_eq!(message_count(&conn), 2);

    fs::write(&path, USER_LINE).unwrap();
    let IngestOutcome::Ingested {
        generation: truncate_generation,
        change: ChangeClass::Truncated,
        ..
    } = ingest(&conn, &path)
    else {
        panic!("shorter content must be classified as truncation");
    };
    assert_eq!(truncate_generation, 2);
    assert_eq!(generation(&conn), 2);
    assert_eq!(
        message_count(&conn),
        1,
        "stale appended rows must be removed"
    );

    let provenance: i64 = conn
        .query_row(
            "SELECT count(*) FROM message WHERE source_artifact_id IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(provenance, 1);
}

#[test]
fn archive_move_keeps_one_source_and_records_both_paths() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("rollout.jsonl");
    let archive_dir = dir.path().join("archived_sessions");
    let archived = archive_dir.join("rollout.jsonl");
    let conn = lore_core::storage::open_in_memory().unwrap();

    fs::create_dir(&archive_dir).unwrap();
    fs::write(&original, USER_LINE).unwrap();
    let first = ingest(&conn, &original);
    assert!(matches!(first, IngestOutcome::Ingested { .. }));

    fs::rename(&original, &archived).unwrap();
    assert_eq!(ingest(&conn, &archived), IngestOutcome::Skipped);

    let sources: i64 = conn
        .query_row("SELECT count(*) FROM source_artifact", [], |row| row.get(0))
        .unwrap();
    let aliases: i64 = conn
        .query_row("SELECT count(*) FROM source_artifact_path", [], |row| {
            row.get(0)
        })
        .unwrap();
    let current_path: String = conn
        .query_row("SELECT current_path FROM source_artifact", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(sources, 1);
    assert_eq!(aliases, 2);
    assert_eq!(current_path, archived.to_string_lossy());
}

#[test]
fn stale_parser_version_forces_reparse_without_generation_bump() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let conn = lore_core::storage::open_in_memory().unwrap();

    fs::write(&path, USER_LINE).unwrap();
    ingest(&conn, &path);
    conn.execute("UPDATE ingest_state SET parser_version = 'old'", [])
        .unwrap();

    let IngestOutcome::Ingested {
        generation,
        change: ChangeClass::Unchanged,
        ..
    } = ingest(&conn, &path)
    else {
        panic!("a parser-version change must reparse unchanged content");
    };
    assert_eq!(generation, 0);
    let parser_version: String = conn
        .query_row("SELECT parser_version FROM ingest_state", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(parser_version, "2");
}

#[test]
fn checkpoint_stops_before_partial_final_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let conn = lore_core::storage::open_in_memory().unwrap();
    let content = format!("{USER_LINE}{{\"type\":");

    fs::write(&path, &content).unwrap();
    ingest(&conn, &path);

    let (last_offset, last_line, source_size): (i64, i64, i64) = conn
        .query_row(
            "SELECT ist.last_offset, ist.last_line, sa.size
             FROM ingest_state ist JOIN source_artifact sa ON sa.id = ist.source_artifact_id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(last_offset, i64::try_from(USER_LINE.len()).unwrap());
    assert_eq!(last_line, 1);
    assert!(
        last_offset < source_size,
        "partial bytes must remain uncheckpointed"
    );
}

#[test]
fn copied_source_preserves_both_links_to_one_logical_session() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.jsonl");
    let copy = dir.path().join("copy.jsonl");
    let conn = lore_core::storage::open_in_memory().unwrap();

    fs::write(&first, USER_LINE).unwrap();
    ingest(&conn, &first);
    fs::copy(&first, &copy).unwrap();
    ingest(&conn, &copy);

    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM agent_session", [], |row| row.get(0))
        .unwrap();
    let sources: i64 = conn
        .query_row("SELECT count(*) FROM source_artifact", [], |row| row.get(0))
        .unwrap();
    let links: i64 = conn
        .query_row("SELECT count(*) FROM session_source", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sessions, 1);
    assert_eq!(sources, 2);
    assert_eq!(links, 2, "replay must preserve the first source link");
}

#[test]
fn rewrite_to_new_native_session_removes_orphaned_logical_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let conn = lore_core::storage::open_in_memory().unwrap();

    fs::write(&path, USER_LINE).unwrap();
    ingest(&conn, &path);
    let replacement = USER_LINE.replace("lifecycle", "new-native");
    fs::write(&path, replacement).unwrap();
    ingest(&conn, &path);

    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM agent_session", [], |row| row.get(0))
        .unwrap();
    let native_id: String = conn
        .query_row("SELECT native_session_id FROM agent_session", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(sessions, 1, "the old source-only session must be removed");
    assert_eq!(native_id, "new-native");
}

#[test]
fn oversized_source_degrades_gracefully_to_partial() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("giant.jsonl");
    let conn = lore_core::storage::open_in_memory().unwrap();

    let mut file = fs::File::create(&path).unwrap();
    use std::io::Write;
    file.write_all(USER_LINE.as_bytes()).unwrap();
    // Sparse resize to 65 MiB (above MAX_SOURCE_BYTES cap)
    file.set_len(65 * 1024 * 1024).unwrap();
    drop(file);

    let outcome = ingest(&conn, &path);
    assert!(matches!(outcome, IngestOutcome::Ingested { .. }));

    let (status, note): (String, Option<String>) = conn
        .query_row(
            "SELECT parse_status, parse_note FROM agent_session",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "partial");
    assert!(
        note.as_deref()
            .unwrap_or("")
            .contains("exceeds maximum supported size"),
        "must record content-free size cap diagnostic"
    );
}
