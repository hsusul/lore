//! Ingest writes the FTS index itself, once per transaction (migration 0015).
//! After every way rows reach or leave `search_document`, a search finds
//! exactly the rows it holds, and FTS5 agrees its index matches that table.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::forget::forget_session;
use lore_core::ingest::{ingest_file, ChangeClass, IngestOutcome};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;
use serde_json::json;

/// The text of each row the index returns for `word`, in rowid order. `None`
/// is an index entry whose row is gone.
fn hits(conn: &Connection, word: &str) -> Vec<Option<String>> {
    let mut stmt = conn
        .prepare(
            "SELECT search_document.redacted_text FROM search_fts
             LEFT JOIN search_document ON search_document.id = search_fts.rowid
             WHERE search_fts MATCH ?1 ORDER BY search_fts.rowid",
        )
        .unwrap();
    let rows = stmt
        .query_map([word], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows
}

fn hit(text: &str) -> Vec<Option<String>> {
    vec![Some(text.to_owned())]
}

/// FTS5's integrity check. The `rank = 1` form also compares the index with
/// the external content table; the plain form only checks the index itself.
fn integrity(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "INSERT INTO search_fts(search_fts) VALUES('integrity-check');
         INSERT INTO search_fts(search_fts, rank) VALUES('integrity-check', 1);",
    )
}

fn line(session: &str, event: serde_json::Value) -> String {
    let mut v = json!({"sessionId": session, "cwd": "/repo/app",
        "timestamp": "2026-08-10T10:00:00.000Z"});
    v.as_object_mut()
        .unwrap()
        .extend(event.as_object().unwrap().clone());
    format!("{v}\n")
}

fn user(session: &str, uuid: &str, text: &str) -> String {
    line(
        session,
        json!({"type": "user", "uuid": uuid, "parentUuid": null,
            "message": {"role": "user", "content": text}}),
    )
}

fn assistant(session: &str, uuid: &str, parent: &str, text: &str) -> String {
    line(
        session,
        json!({"type": "assistant", "uuid": uuid, "parentUuid": parent,
            "message": {"role": "assistant", "model": "claude-x",
                "content": [{"type": "text", "text": text}]}}),
    )
}

fn custom_title(session: &str, title: &str) -> String {
    line(
        session,
        json!({"type": "custom-title", "customTitle": title}),
    )
}

struct Archive {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    blobs: BlobStore,
    conn: Connection,
}

impl Archive {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let blobs = BlobStore::open(root.join("blobs")).unwrap();
        let conn = lore_core::storage::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TEMP TABLE replaced (session_id TEXT);
             CREATE TEMP TRIGGER log_replace AFTER DELETE ON agent_session
             BEGIN INSERT INTO replaced VALUES (old.id); END;",
        )
        .unwrap();
        Archive {
            _dir: dir,
            root,
            blobs,
            conn,
        }
    }

    /// Write `content` to `file` and ingest it. Returns the session id, the
    /// change class, and whether ingest replaced an existing session.
    fn ingest(&self, file: &str, content: &str) -> (String, ChangeClass, bool) {
        let path = self.root.join(file);
        std::fs::write(&path, content).unwrap();
        self.conn.execute_batch("DELETE FROM replaced").unwrap();
        let outcome = ingest_file(&self.conn, &ClaudeCodeAdapter::new(), &path, &self.blobs);
        let replaced: i64 = self
            .conn
            .query_row("SELECT count(*) FROM replaced", [], |r| r.get(0))
            .unwrap();
        let (session_id, change) = match outcome.unwrap() {
            IngestOutcome::Ingested {
                session_id, change, ..
            } => Some((session_id, change)),
            IngestOutcome::Skipped => None,
        }
        .expect("a changed file must be ingested");
        (session_id, change, replaced > 0)
    }
}

#[test]
fn the_content_check_catches_a_row_missing_from_the_index() {
    let archive = Archive::new();
    let (session, _, _) = archive.ingest("a.jsonl", &user("s-a", "u1", "Tune the quokka cache"));
    integrity(&archive.conn).unwrap();
    archive
        .conn
        .execute(
            "INSERT INTO search_document
                (session_id, source_kind, source_id, field, redacted_text, created_at)
             VALUES (?1, 'session', ?1, 'title', 'An unindexed wombat', 1)",
            [&session],
        )
        .unwrap();
    assert_eq!(hits(&archive.conn, "wombat"), Vec::<Option<String>>::new());
    assert!(integrity(&archive.conn).is_err());
}

#[test]
fn search_matches_the_archive_through_ingest_append_replace_and_forget() {
    let archive = Archive::new();
    let first = user("s-a", "u1", "Tune the quokka cache");
    let second = assistant("s-a", "a1", "u1", "The wombat route is cold");
    let third = user("s-a", "u2", "Restart the numbat worker");

    let (session, change, _) = archive.ingest("a.jsonl", &format!("{first}{second}"));
    assert_eq!(change, ChangeClass::New);
    assert_eq!(hits(&archive.conn, "quokka"), hit("Tune the quokka cache"));
    assert_eq!(
        hits(&archive.conn, "wombat"),
        hit("The wombat route is cold")
    );
    integrity(&archive.conn).unwrap();

    let (appended, change, replaced) =
        archive.ingest("a.jsonl", &format!("{first}{second}{third}"));
    assert_eq!(
        (appended.as_str(), change, replaced),
        (session.as_str(), ChangeClass::Appended, false)
    );
    assert_eq!(hits(&archive.conn, "quokka"), hit("Tune the quokka cache"));
    assert_eq!(
        hits(&archive.conn, "wombat"),
        hit("The wombat route is cold")
    );
    assert_eq!(
        hits(&archive.conn, "numbat"),
        hit("Restart the numbat worker")
    );
    integrity(&archive.conn).unwrap();

    let rewritten = user("s-a", "u1", "Tune the platypus cache");
    let (replaced_id, change, replaced) =
        archive.ingest("a.jsonl", &format!("{rewritten}{second}{third}"));
    assert_eq!(
        (replaced_id.as_str(), change, replaced),
        (session.as_str(), ChangeClass::Rewritten, true)
    );
    assert_eq!(hits(&archive.conn, "quokka"), Vec::<Option<String>>::new());
    assert_eq!(
        hits(&archive.conn, "platypus"),
        hit("Tune the platypus cache")
    );
    assert_eq!(
        hits(&archive.conn, "wombat"),
        hit("The wombat route is cold")
    );
    assert_eq!(
        hits(&archive.conn, "numbat"),
        hit("Restart the numbat worker")
    );
    integrity(&archive.conn).unwrap();

    archive.ingest("b.jsonl", &user("s-b", "u1", "Plant the kiwi seeds"));
    forget_session(&archive.conn, &archive.blobs, &session).unwrap();
    for word in ["platypus", "wombat", "numbat"] {
        assert_eq!(
            hits(&archive.conn, word),
            Vec::<Option<String>>::new(),
            "{word}"
        );
    }
    assert_eq!(hits(&archive.conn, "kiwi"), hit("Plant the kiwi seeds"));
    integrity(&archive.conn).unwrap();
}

#[test]
fn a_source_rewritten_into_another_session_drops_the_old_sessions_rows() {
    let archive = Archive::new();
    let (old, _, _) = archive.ingest("a.jsonl", &user("s-a", "u1", "Tune the quokka cache"));
    let (new, change, _) = archive.ingest("a.jsonl", &user("s-z", "u1", "Tune the platypus cache"));
    assert_ne!(new, old);
    assert_eq!(change, ChangeClass::Rewritten);
    assert_eq!(hits(&archive.conn, "quokka"), Vec::<Option<String>>::new());
    assert_eq!(
        hits(&archive.conn, "platypus"),
        hit("Tune the platypus cache")
    );
    integrity(&archive.conn).unwrap();
}

/// The title is the archive's highest search row here, so deleting it frees
/// that id for the new title an append writes.
#[test]
fn an_appended_title_change_indexes_the_new_title() {
    let archive = Archive::new();
    let thinking = line(
        "s-t",
        json!({"type": "assistant", "uuid": "a1", "parentUuid": null,
            "message": {"role": "assistant", "model": "claude-x",
                "content": [{"type": "thinking", "thinking": "hidden", "signature": "sig"}]}}),
    );
    let head = format!("{}{thinking}", custom_title("s-t", "Quokka plans"));
    archive.ingest("t.jsonl", &head);
    assert_eq!(hits(&archive.conn, "quokka"), hit("Quokka plans"));

    let tail = format!(
        "{}{}",
        custom_title("s-t", "Numbat plans"),
        user("s-t", "u1", "Feed the wombat")
    );
    let (_, change, replaced) = archive.ingest("t.jsonl", &format!("{head}{tail}"));
    assert_eq!((change, replaced), (ChangeClass::Appended, false));
    assert_eq!(hits(&archive.conn, "quokka"), Vec::<Option<String>>::new());
    assert_eq!(hits(&archive.conn, "numbat"), hit("Numbat plans"));
    assert_eq!(hits(&archive.conn, "wombat"), hit("Feed the wombat"));
    integrity(&archive.conn).unwrap();
}

/// A schema-14 archive is this schema plus the per-row insert trigger, with an
/// index already in sync. Migration 0015 must leave it in sync, and later
/// writes must keep it so.
#[test]
fn an_archive_indexed_by_the_insert_trigger_stays_in_sync_after_migration_0015() {
    let archive = Archive::new();
    let quokka = user("s-a", "u1", "Tune the quokka cache");
    archive.ingest("a.jsonl", &quokka);
    archive
        .conn
        .execute_batch(
            "CREATE TRIGGER search_document_ai AFTER INSERT ON search_document BEGIN
                 INSERT INTO search_fts(rowid, redacted_text) VALUES (new.id, new.redacted_text);
             END;
             DELETE FROM schema_migrations WHERE version = 15;
             INSERT INTO agent (id, display_name) VALUES ('old', 'Old');
             INSERT INTO agent_session (id, agent_id, dedupe_key) VALUES ('s-old', 'old', 'k');
             INSERT INTO search_document
                 (session_id, source_kind, source_id, field, redacted_text, created_at)
             VALUES ('s-old', 'session', 's-old', 'title', 'Migrate the axolotl ledger', 1);",
        )
        .unwrap();
    assert_eq!(
        hits(&archive.conn, "axolotl"),
        hit("Migrate the axolotl ledger")
    );
    integrity(&archive.conn).unwrap();

    lore_core::storage::migrations::run(&archive.conn).unwrap();
    assert_eq!(hits(&archive.conn, "quokka"), hit("Tune the quokka cache"));
    assert_eq!(
        hits(&archive.conn, "axolotl"),
        hit("Migrate the axolotl ledger")
    );
    integrity(&archive.conn).unwrap();

    archive.ingest("b.jsonl", &user("s-b", "u1", "Plant the kiwi seeds"));
    assert_eq!(hits(&archive.conn, "kiwi"), hit("Plant the kiwi seeds"));
    let (_, change, _) = archive.ingest("a.jsonl", &user("s-a", "u1", "Tune the platypus cache"));
    assert_eq!(change, ChangeClass::Rewritten);
    assert_eq!(hits(&archive.conn, "quokka"), Vec::<Option<String>>::new());
    assert_eq!(
        hits(&archive.conn, "platypus"),
        hit("Tune the platypus cache")
    );
    forget_session(&archive.conn, &archive.blobs, "s-old").unwrap();
    assert_eq!(hits(&archive.conn, "axolotl"), Vec::<Option<String>>::new());
    integrity(&archive.conn).unwrap();
}
