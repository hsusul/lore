//! Attachment bodies are stored once (migration 0019, `lore_core::compact`):
//! new ingests write each distinct body once, older rows are converted by a
//! verified, resumable migration, forget and re-ingest leave no orphaned body,
//! and every reader still gets the exact bytes that were ingested.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::AtomicBool;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::compact::{self, VacuumOutcome, VacuumSpace};
use lore_core::export::export_session_markdown;
use lore_core::forget::forget_session;
use lore_core::ingest::persist_session;
use lore_core::search::search;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

const SHARED: &str = "shared project context: retryBackoff lives in net.rs";
const ONLY_A: &str = "context only session a carries";
const ONLY_B: &str = "context only session b carries";

fn store() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    (dir, s)
}

/// A Claude Code transcript: one user prompt, then one attachment event per
/// body, the way Claude Code records injected context.
fn transcript(session: &str, bodies: &[&str]) -> String {
    let mut out = format!(
        "{{\"type\":\"user\",\"uuid\":\"u0_{session}\",\"sessionId\":\"{session}\",\"cwd\":\"/p\",\
         \"timestamp\":\"2026-08-11T10:00:00.000Z\",\
         \"message\":{{\"role\":\"user\",\"content\":\"fix the retryBackoff jitter\"}}}}\n"
    );
    for (i, body) in bodies.iter().enumerate() {
        out.push_str(&format!(
            "{{\"type\":\"attachment\",\"uuid\":\"u{n}_{session}\",\"sessionId\":\"{session}\",\
             \"timestamp\":\"2026-08-11T10:00:0{n}.000Z\",\
             \"attachment\":{{\"type\":\"prompt_snapshot\",\"content\":\"{body}\"}}}}\n",
            n = i + 1
        ));
    }
    out
}

fn persist(conn: &Connection, blobs: &BlobStore, session: &str, bodies: &[&str]) -> String {
    let parsed = ClaudeCodeAdapter::new().parse_str(&transcript(session, bodies), session);
    persist_session(conn, "claude-code", "Claude Code", &parsed, blobs).unwrap()
}

/// Two sessions sharing one body: four attachment rows, three distinct bodies.
fn two_sessions(conn: &Connection, blobs: &BlobStore) -> (String, String) {
    let a = persist(conn, blobs, "sa", &[SHARED, ONLY_A]);
    let b = persist(conn, blobs, "sb", &[SHARED, ONLY_B]);
    (a, b)
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Every attachment body as the one read path serves it, in row order.
fn served_attachments(conn: &Connection) -> Vec<(String, String)> {
    conn.prepare(
        "SELECT v.id, v.content_json FROM message_part_full v JOIN message_part p ON p.id = v.id
         WHERE v.kind = 'attachment' ORDER BY p.rowid",
    )
    .unwrap()
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

/// Put the archive back in the shape an archive ingested before migration
/// 0019 has: every attachment body inline, `part_body` empty.
fn make_legacy(conn: &Connection) {
    conn.execute_batch(
        "UPDATE message_part
            SET content_json = (SELECT body FROM part_body WHERE hash = message_part.body_hash),
                body_hash = NULL
          WHERE body_hash IS NOT NULL;
         DELETE FROM part_body;",
    )
    .unwrap();
}

/// What a reader sees: each session's Markdown export (with flagged content,
/// so nothing is masked away) and the hits of a few searches.
fn reader_view(conn: &Connection, sessions: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = sessions
        .iter()
        .map(|s| export_session_markdown(conn, s, true).unwrap().unwrap())
        .collect();
    for q in ["retryBackoff", "jitter", "context", "net.rs"] {
        out.push(format!("{:?}", search(conn, q, 50).unwrap()));
    }
    out
}

#[test]
fn ingest_stores_each_attachment_body_once() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    two_sessions(&conn, &blobs);

    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message_part WHERE kind = 'attachment'"
        ),
        4
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM part_body"),
        3,
        "one row per body"
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message_part
             WHERE kind = 'attachment' AND (content_json IS NOT NULL OR body_hash IS NULL)"
        ),
        0,
        "no attachment body is kept inline"
    );

    // Byte-equal to what the adapter parsed, through the one read path.
    let parsed = ClaudeCodeAdapter::new().parse_str(&transcript("sa", &[SHARED, ONLY_A]), "sa");
    let expected: Vec<String> = parsed
        .messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter(|p| p.kind.as_str() == "attachment")
        .map(|p| p.content_json.clone().unwrap())
        .collect();
    let served: Vec<String> = served_attachments(&conn)
        .into_iter()
        .take(2)
        .map(|(_, body)| body)
        .collect();
    assert_eq!(served, expected);
}

#[test]
fn the_migration_converts_duplicates_to_one_body_and_reads_back_byte_equal() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let (a, b) = two_sessions(&conn, &blobs);
    make_legacy(&conn);
    let before = served_attachments(&conn);
    let view_before = reader_view(&conn, &[&a, &b]);
    assert_eq!(compact::inline_attachments(&conn).unwrap(), 4);

    // A batch of one exercises the resume path between every row.
    let report = compact::dedupe_attachments(&conn, 1, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.converted, 4);
    assert_eq!(report.bodies_added, 3);
    assert_eq!(report.remaining, 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM part_body"), 3);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM message_part WHERE content_json IS NOT NULL AND kind = 'attachment'"),
        0
    );

    assert_eq!(
        served_attachments(&conn),
        before,
        "bodies read back byte-equal"
    );
    assert_eq!(
        reader_view(&conn, &[&a, &b]),
        view_before,
        "export and search unchanged"
    );

    // Rerunning is a no-op.
    let again = compact::dedupe_attachments(&conn, 1, &AtomicBool::new(false)).unwrap();
    assert_eq!(again, compact::DedupeReport::default());
    assert_eq!(served_attachments(&conn), before);
}

#[test]
fn a_stop_request_leaves_the_rest_for_the_next_run() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    two_sessions(&conn, &blobs);
    make_legacy(&conn);
    let before = served_attachments(&conn);

    let stopped = compact::dedupe_attachments(&conn, 1, &AtomicBool::new(true)).unwrap();
    assert_eq!((stopped.converted, stopped.remaining), (0, 4));
    let resumed = compact::dedupe_attachments(&conn, 1, &AtomicBool::new(false)).unwrap();
    assert_eq!((resumed.converted, resumed.remaining), (4, 0));
    assert_eq!(served_attachments(&conn), before);
}

#[test]
fn a_mismatch_stops_the_migration_and_leaves_that_row_intact() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    two_sessions(&conn, &blobs);
    make_legacy(&conn);
    let before = served_attachments(&conn);

    // A damaged part_body row: it claims the second row's hash but holds
    // other bytes. The second row must not be pointed at it.
    let (victim_id, victim_body) = before[1].clone();
    let victim_hash = compact::body_hash(&victim_body);
    conn.execute(
        "INSERT INTO part_body (hash, body) VALUES (?1, 'not the original bytes')",
        [&victim_hash],
    )
    .unwrap();

    let err = compact::dedupe_attachments(&conn, 10, &AtomicBool::new(false)).unwrap_err();
    assert!(err.to_string().contains(&victim_id), "{err}");

    // The victim is exactly as it was: inline, unpointed, same bytes.
    let (inline, hash): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT content_json, body_hash FROM message_part WHERE id = ?1",
            [&victim_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(inline.as_deref(), Some(victim_body.as_str()));
    assert_eq!(hash, None);
    // The row before it was converted and committed; nothing after it was.
    assert_eq!(compact::inline_attachments(&conn).unwrap(), 3);
    assert_eq!(
        served_attachments(&conn),
        before,
        "no reader sees a changed byte"
    );
}

#[test]
fn ingest_never_points_a_part_at_a_body_that_differs() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // Find the exact body the adapter stores for ONLY_A, then damage its slot.
    let probe = lore_core::storage::open_in_memory().unwrap();
    persist(&probe, &blobs, "sa", &[ONLY_A]);
    let body = served_attachments(&probe).remove(0).1;
    conn.execute(
        "INSERT INTO part_body (hash, body) VALUES (?1, 'damaged')",
        [compact::body_hash(&body)],
    )
    .unwrap();

    persist(&conn, &blobs, "sa", &[ONLY_A]);
    assert_eq!(
        served_attachments(&conn).remove(0).1,
        body,
        "kept inline instead"
    );
}

#[test]
fn forget_removes_a_sessions_unique_bodies_and_keeps_shared_ones() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let (a, b) = two_sessions(&conn, &blobs);
    let export_b = export_session_markdown(&conn, &b, true).unwrap();

    forget_session(&conn, &blobs, &a).unwrap();

    let mut left: Vec<String> = conn
        .prepare("SELECT hash FROM part_body ORDER BY hash")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    left.sort();
    let mut expected: Vec<String> = served_attachments(&conn)
        .into_iter()
        .map(|(_, body)| compact::body_hash(&body))
        .collect();
    expected.sort();
    assert_eq!(left, expected, "only session b's bodies remain");
    assert_eq!(left.len(), 2, "the shared body and b's own");
    assert_eq!(export_session_markdown(&conn, &b, true).unwrap(), export_b);

    forget_session(&conn, &blobs, &b).unwrap();
    assert_eq!(count(&conn, "SELECT count(*) FROM part_body"), 0);
}

#[test]
fn forget_all_removes_every_body() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    two_sessions(&conn, &blobs);
    lore_core::forget::forget_all(&conn, &blobs).unwrap();
    assert_eq!(count(&conn, "SELECT count(*) FROM part_body"), 0);
}

#[test]
fn a_reingest_drops_the_bodies_the_new_rows_no_longer_use() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    persist(&conn, &blobs, "sa", &[SHARED, ONLY_A]);
    persist(&conn, &blobs, "sa", &[SHARED, ONLY_B]);
    assert_eq!(count(&conn, "SELECT count(*) FROM part_body"), 2);
    let served: Vec<String> = served_attachments(&conn)
        .into_iter()
        .map(|(_, b)| b)
        .collect();
    assert!(served.iter().all(|b| !b.contains(ONLY_A)));
}

/// A file-backed archive and the blob store beside it.
fn file_archive(dir: &std::path::Path) -> (Connection, BlobStore) {
    let conn = lore_core::storage::open(&dir.join("lore.db")).unwrap();
    let blobs = BlobStore::open(dir.join("blobs")).unwrap();
    (conn, blobs)
}

fn backups(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    lore_core::backup::list_backups(&dir.join(lore_core::paths::BACKUPS_DIRNAME)).unwrap()
}

#[test]
fn compaction_backs_up_before_its_first_batch() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = file_archive(dir.path());
    two_sessions(&conn, &blobs);
    make_legacy(&conn);
    assert!(backups(dir.path()).is_empty());

    let report = compact::compact(&conn, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.dedupe.converted, 4);
    let taken = backups(dir.path());
    assert_eq!(taken.len(), 1, "backed up first");
    let old = Connection::open(&taken[0]).unwrap();
    assert_eq!(
        count(&old, "SELECT count(*) FROM message_part WHERE content_json IS NOT NULL AND kind = 'attachment'"),
        4,
        "the backup holds the rows as they were"
    );

    compact::compact(&conn, &AtomicBool::new(false)).unwrap();
    assert_eq!(backups(dir.path()).len(), 1, "nothing left to convert");
}

#[test]
fn compaction_converts_nothing_without_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = file_archive(dir.path());
    two_sessions(&conn, &blobs);
    make_legacy(&conn);
    std::fs::write(dir.path().join(lore_core::paths::BACKUPS_DIRNAME), b"").unwrap();

    let err = compact::compact(&conn, &AtomicBool::new(false)).unwrap_err();
    assert!(err.to_string().contains("back up"), "{err}");
    assert_eq!(compact::inline_attachments(&conn).unwrap(), 4);
}

#[test]
fn a_new_archive_is_incremental_and_forget_returns_its_pages() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, blobs) = file_archive(dir.path());
    assert!(compact::is_incremental(&conn).unwrap());

    let big = "forgotten words ".repeat(30_000);
    persist(&conn, &blobs, "keep", &[SHARED]);
    let gone = persist(&conn, &blobs, "gone", &[&big]);
    let pages_before: i64 = count(&conn, "PRAGMA page_count");

    forget_session(&conn, &blobs, &gone).unwrap();
    assert_eq!(
        compact::freelist_count(&conn).unwrap(),
        0,
        "free pages returned"
    );
    assert!(
        count(&conn, "PRAGMA page_count") < pages_before,
        "the file shrank"
    );
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
}

#[test]
fn an_old_archive_is_vacuumed_once_and_only_with_room() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lore.db");
    // An archive created before Lore set auto_vacuum.
    {
        let raw = Connection::open(&path).unwrap();
        raw.execute_batch("PRAGMA auto_vacuum = NONE; CREATE TABLE early (x INTEGER);")
            .unwrap();
    }
    let (conn, blobs) = file_archive(dir.path());
    assert!(!compact::is_incremental(&conn).unwrap(), "needs the VACUUM");
    let (a, b) = two_sessions(&conn, &blobs);
    make_legacy(&conn);
    let view_before = reader_view(&conn, &[&a, &b]);

    assert_eq!(
        compact::vacuum_once_with(&conn, VacuumSpace::one_volume(u64::MAX)).unwrap(),
        VacuumOutcome::DedupePending,
        "the file is rewritten once, after the conversion"
    );
    compact::dedupe_attachments(&conn, 100, &AtomicBool::new(false)).unwrap();

    let skipped = compact::vacuum_once_with(&conn, VacuumSpace::one_volume(1024)).unwrap();
    assert!(matches!(
        skipped,
        VacuumOutcome::LowSpace {
            available: 1024,
            ..
        }
    ));
    assert!(!compact::is_incremental(&conn).unwrap());
    assert_eq!(
        lore_core::settings::get(&conn, compact::VACUUM_SETTING)
            .unwrap()
            .as_deref(),
        Some("\"skipped_low_space\"")
    );

    let done = compact::vacuum_once_with(&conn, VacuumSpace::one_volume(u64::MAX)).unwrap();
    assert!(matches!(done, VacuumOutcome::Vacuumed { .. }), "{done:?}");
    assert!(compact::is_incremental(&conn).unwrap());
    assert_eq!(
        count(&conn, "PRAGMA temp_store"),
        2,
        "back to MEMORY after the VACUUM wrote its copy to a file"
    );
    assert_eq!(
        compact::vacuum_once_with(&conn, VacuumSpace::one_volume(u64::MAX)).unwrap(),
        VacuumOutcome::AlreadyIncremental
    );
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
    assert_eq!(reader_view(&conn, &[&a, &b]), view_before);

    // Now in incremental mode, a forget hands its pages back.
    let big = "forgotten words ".repeat(30_000);
    let gone = persist(&conn, &blobs, "gone", &[&big]);
    forget_session(&conn, &blobs, &gone).unwrap();
    assert_eq!(compact::freelist_count(&conn).unwrap(), 0);
}
