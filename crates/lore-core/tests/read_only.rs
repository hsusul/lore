//! The read-only archive boundary, exercised against a real writer.
//!
//! One of the alpha's architectural bets is that a second process can query the
//! archive while the desktop worker is ingesting: reads never block, never see
//! "database is locked", and never need `storage::write_lock`. That is a claim
//! about SQLite's WAL behaviour under Lore's actual configuration, so it is
//! tested with the real writer path rather than a mock.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lore_core::storage::StorageError;
use lore_core::{folders, storage};

fn is_locked(message: &str) -> bool {
    message.contains("locked") || message.contains("busy")
}

fn folder_count(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM folder", [], |row| row.get(0))
        .unwrap()
}

/// A reader keeps working, without the write mutex, while a real writer commits
/// continuously to the same file.
///
/// The overlap is asserted from a *delta* measured across the read loop. An
/// earlier version asserted `written > 0` after a pre-wait that already
/// guaranteed it, so it would have passed even if the writer had stopped before
/// the first read — it proved nothing about concurrency.
#[test]
fn read_only_reader_is_unblocked_by_a_live_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lore.db");
    drop(storage::open(&path).unwrap());

    let stop = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicU64::new(0));
    let writer_path = path.clone();
    let writer_stop = stop.clone();
    let writer_written = written.clone();

    let writer = std::thread::spawn(move || {
        let conn = storage::open(&writer_path).unwrap();
        while !writer_stop.load(Ordering::Relaxed) {
            if folders::create_folder(&conn, "w").is_ok() {
                writer_written.fetch_add(1, Ordering::Relaxed);
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while written.load(Ordering::Relaxed) < 5 && Instant::now() < deadline {
        std::thread::yield_now();
    }

    let reader = storage::open_read_only(&path).unwrap();
    let at_start = written.load(Ordering::Relaxed);
    let mut locked = 0u64;
    let mut previous = 0i64;
    let mut reads = 0u64;

    // Read until the writer has committed a further 200 rows, so the loop
    // provably spans live write traffic rather than a fixed count that might
    // finish in an idle window. Bounded by a deadline so a stalled writer fails
    // the test instead of hanging it.
    let loop_deadline = Instant::now() + Duration::from_secs(20);
    while written.load(Ordering::Relaxed) < at_start + 200 && Instant::now() < loop_deadline {
        match reader.query_row("SELECT count(*) FROM folder", [], |row| {
            row.get::<_, i64>(0)
        }) {
            Ok(count) => {
                // The writer only adds, so a reader that saw N must never
                // subsequently see fewer. A torn or partially-applied
                // transaction would surface here.
                assert!(
                    count >= previous,
                    "folder count went backwards: {previous} -> {count}"
                );
                previous = count;
                reads += 1;
            }
            Err(error) => {
                if is_locked(&error.to_string()) {
                    locked += 1;
                } else {
                    panic!("unexpected read error: {error}");
                }
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    let delta = written.load(Ordering::Relaxed) - at_start;

    assert_eq!(
        locked, 0,
        "a WAL reader must never be told the db is locked"
    );
    assert!(reads > 0, "the reader must have run");
    assert!(
        delta >= 200,
        "the writer must have committed DURING the read loop, saw {delta}"
    );
    assert!(
        previous >= at_start as i64,
        "the reader must have observed writes made during the loop"
    );
}

/// The strongest form of the claim: a reader is not blocked by a writer's *open*
/// transaction. If the reader took a lock, it would wait out `busy_timeout` and
/// then fail; instead it reads the pre-transaction snapshot immediately.
#[test]
fn a_reader_does_not_wait_on_an_open_write_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lore.db");
    let writer = storage::open(&path).unwrap();

    let tx = writer.unchecked_transaction().unwrap();
    folders::create_folder(&tx, "uncommitted").unwrap();

    let reader = storage::open_read_only(&path).unwrap();
    let started = Instant::now();
    assert_eq!(folder_count(&reader), 0, "uncommitted work is invisible");
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "the reader waited {:?} — it took a lock it should not need",
        started.elapsed()
    );

    tx.commit().unwrap();
    assert_eq!(folder_count(&reader), 1, "committed work becomes visible");
}

/// A reader inside an explicit transaction holds one snapshot, and a concurrent
/// writer — including one running a TRUNCATE checkpoint — cannot change what it
/// sees. Note the direction: readers hold up RESTART/TRUNCATE checkpoints, not
/// the other way round.
#[test]
fn a_readers_snapshot_survives_a_writer_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lore.db");
    let writer = storage::open(&path).unwrap();
    folders::create_folder(&writer, "before").unwrap();

    let reader = storage::open_read_only(&path).unwrap();
    let snapshot = reader.unchecked_transaction().unwrap();
    let seen = folder_count(&snapshot);
    assert_eq!(seen, 1);

    folders::create_folder(&writer, "during").unwrap();
    // A checkpoint that cannot complete while a reader is open reports busy in
    // its result row rather than erroring; either way the snapshot must hold.
    let _ = writer.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");

    assert_eq!(
        folder_count(&snapshot),
        seen,
        "an open reader's snapshot must not change under it"
    );
    drop(snapshot);
    assert_eq!(folder_count(&reader), 2, "a new statement sees the new row");
}

/// The reader does not participate in `storage::write_lock`. Holding that mutex
/// for the whole read loop would deadlock if it did.
#[test]
fn read_only_reader_does_not_need_the_process_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lore.db");
    {
        let writer = storage::open(&path).unwrap();
        folders::create_folder(&writer, "seeded").unwrap();
    }

    let reader = storage::open_read_only(&path).unwrap();
    let guard = storage::write_lock();
    for _ in 0..100 {
        assert_eq!(folder_count(&reader), 1);
    }
    drop(guard);
}

/// A reader opened *before* a write still observes it: WAL readers start a new
/// snapshot per statement outside an explicit transaction.
#[test]
fn a_reader_opened_first_observes_later_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lore.db");
    let writer = storage::open(&path).unwrap();

    let reader = storage::open_read_only(&path).unwrap();
    assert_eq!(folder_count(&reader), 0);

    folders::create_folder(&writer, "after").unwrap();
    assert_eq!(folder_count(&reader), 1);
}

/// End to end through the path helper the CLI will use: resolve, open writable,
/// close, reopen read-only.
#[test]
fn an_archive_created_by_the_writer_reopens_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = lore_core::paths::archive_db_path(Some(dir.path())).unwrap();
    {
        let writer = storage::open(&db).unwrap();
        folders::create_folder(&writer, "persisted").unwrap();
    }

    let reader = storage::open_read_only(&db).unwrap();
    assert_eq!(folder_count(&reader), 1);

    let error = folders::create_folder(&reader, "nope").unwrap_err();
    assert!(
        matches!(
            error,
            StorageError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ReadOnly
        ),
        "a domain write through a read-only connection must fail as read-only, got {error:?}"
    );
}

// ── The query surface over a read-only archive ──────────────────────────────

/// Build a real archive on disk — a Codex session whose recorded changes carry
/// patch payloads — and return its paths plus the session id.
///
/// Written with the ordinary writer path, then closed. Everything after this is
/// a reader's view of a finished archive, which is the situation a query surface
/// is actually in.
fn seeded_archive() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    String,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lore.db");
    let blob_dir = dir.path().join("blobs");

    let fixture = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/codex/patch_apply.jsonl"),
    )
    .unwrap();

    // A second session carrying ordinary prose. The codex patch fixture is all
    // tool calls and patch payloads, so nothing of it reaches the search
    // projection — a search test against it alone can only ever assert
    // "returned no error", which is what made the previous version a tautology.
    let claude = concat!(
        r#"{"type":"user","uuid":"u1","sessionId":"ro-text","cwd":"/proj","#,
        r#""message":{"role":"user","content":"the quokka refactor is finished"}}"#,
        "\n"
    );

    let sid = {
        let conn = storage::open(&db).unwrap();
        let blobs = lore_core::storage::blob::BlobStore::open(&blob_dir).unwrap();
        let parsed = lore_core::adapters::codex::CodexAdapter::new().parse_str(&fixture, "ro");
        let sid =
            lore_core::ingest::persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();
        let text =
            lore_core::adapters::claude_code::ClaudeCodeAdapter::new().parse_str(claude, "ro-text");
        lore_core::ingest::persist_session(&conn, "claude-code", "Claude Code", &text, &blobs)
            .unwrap();
        sid
    };
    (dir, db, blob_dir, sid)
}

#[test]
fn a_session_reads_back_through_a_read_only_connection() {
    let (_dir, db, _blobs, sid) = seeded_archive();
    let reader = storage::open_read_only(&db).unwrap();

    let detail = lore_core::query::get_session(&reader, &sid)
        .unwrap()
        .expect("the persisted session is readable");
    assert_eq!(detail.summary.id, sid);
    assert!(
        !detail.messages.is_empty(),
        "a read-only connection returned no messages"
    );
    assert!(
        !detail.file_events.is_empty(),
        "the fixture's recorded file changes are readable"
    );
}

#[test]
fn search_pages_through_a_read_only_connection() {
    // Search touches the FTS tables, which is the read most likely to want a
    // write (temp indexes, spill). It must not.
    let (_dir, db, _blobs, _sid) = seeded_archive();
    let reader = storage::open_read_only(&db).unwrap();

    // A term the seeded prose actually contains, so a zero-hit page fails.
    let page = lore_core::search::search_page(
        &reader,
        "quokka",
        10,
        None,
        lore_core::search::SortOrder::Relevance,
    )
    .unwrap();
    assert!(
        !page.hits.is_empty(),
        "read-only search returned nothing for a term the archive contains"
    );
}

#[test]
fn a_patch_reads_back_without_creating_a_blob_store() {
    // `inspect --patch` in one line: read a recorded patch out of an archive
    // while leaving the archive exactly as it was found.
    let (_dir, db, blob_dir, sid) = seeded_archive();
    let reader = storage::open_read_only(&db).unwrap();
    let blobs = lore_core::storage::blob::BlobStore::open_existing(&blob_dir).unwrap();

    let event_id: String = reader
        .query_row(
            "SELECT id FROM file_event WHERE session_id = ?1 AND patch_blob_id IS NOT NULL
             ORDER BY id LIMIT 1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();

    let patch = lore_core::query::file_patch_text(&reader, &blobs, &event_id)
        .unwrap()
        .expect("the recorded patch is readable");
    assert!(!patch.is_empty());
}

#[test]
fn reading_an_archive_leaves_its_directory_untouched() {
    // The property a query surface owes an archive it does not own. Compares the
    // full directory listing before and after a read, so a stray `tmp/` — or
    // anything else — fails here.
    let (dir, db, blob_dir, sid) = seeded_archive();

    let listing = |root: &std::path::Path| -> Vec<String> {
        fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                out.push(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
                if path.is_dir() {
                    walk(&path, base, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        // WAL sidecars come and go with connections and say nothing about
        // whether the archive's *contents* were modified.
        out.retain(|p| !p.ends_with("-wal") && !p.ends_with("-shm"));
        out.sort();
        out
    };

    let before = listing(dir.path());
    {
        let reader = storage::open_read_only(&db).unwrap();
        let blobs = lore_core::storage::blob::BlobStore::open_existing(&blob_dir).unwrap();
        lore_core::query::get_session(&reader, &sid).unwrap();
        lore_core::search::search_page(
            &reader,
            "patch",
            10,
            None,
            lore_core::search::SortOrder::Relevance,
        )
        .unwrap();
        if let Ok(event_id) = reader.query_row(
            "SELECT id FROM file_event WHERE session_id = ?1 AND patch_blob_id IS NOT NULL
             ORDER BY id LIMIT 1",
            [&sid],
            |r| r.get::<_, String>(0),
        ) {
            lore_core::query::file_patch_text(&reader, &blobs, &event_id).unwrap();
        }
    }
    assert_eq!(before, listing(dir.path()), "reading changed the archive");
}

#[test]
fn a_writer_opened_blob_store_would_have_left_a_trace() {
    // Guards the previous test against passing for the wrong reason: it must be
    // `open_existing` doing the work, not the read path happening to be inert.
    let dir = tempfile::tempdir().unwrap();
    let blob_dir = dir.path().join("blobs");
    std::fs::create_dir(&blob_dir).unwrap();

    lore_core::storage::blob::BlobStore::open_existing(&blob_dir).unwrap();
    assert!(!blob_dir.join("tmp").exists());

    lore_core::storage::blob::BlobStore::open(&blob_dir).unwrap();
    assert!(
        blob_dir.join("tmp").exists(),
        "the writer constructor is expected to create tmp/ — if it no longer \
         does, `reading_an_archive_leaves_its_directory_untouched` proves less \
         than it appears to"
    );
}
