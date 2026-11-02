//! Re-ingesting a source replaces its session's parsed rows. What the user or
//! Lore recorded about that session, and not the agent log, must survive: a
//! folder assignment, a forget, and when git evidence was first observed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::ingest::{ingest_file, IngestOutcome};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

const SESSION: &str = "12121212-0000-4000-8000-000000000001";

fn line(uuid: &str, role: &str, text: &str) -> String {
    format!(
        r#"{{"type":"{role}","uuid":"{uuid}","sessionId":"{SESSION}","timestamp":"2026-10-31T18:00:00.000Z","cwd":"/nonexistent/lore-test","gitBranch":"main","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    ) + "\n"
}

struct Archive {
    conn: Connection,
    blobs: BlobStore,
    _blob_dir: tempfile::TempDir,
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl Archive {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{SESSION}.jsonl"));
        std::fs::write(
            &path,
            line("u1", "user", "question") + &line("u2", "assistant", "answer names the larch"),
        )
        .unwrap();
        let blob_dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(blob_dir.path()).unwrap();
        let archive = Self {
            conn: lore_core::storage::open_in_memory().unwrap(),
            blobs,
            _blob_dir: blob_dir,
            _dir: dir,
            path,
        };
        assert!(matches!(archive.ingest(), IngestOutcome::Ingested { .. }));
        archive
    }

    fn ingest(&self) -> IngestOutcome {
        ingest_file(
            &self.conn,
            &ClaudeCodeAdapter::new(),
            &self.path,
            &self.blobs,
        )
        .unwrap()
    }

    fn session_id(&self) -> Option<String> {
        self.conn
            .query_row(
                "SELECT id FROM agent_session WHERE native_session_id = ?1",
                [SESSION],
                |r| r.get(0),
            )
            .ok()
    }

    /// Mark every source as parsed by the previous parser, as an archive
    /// written before a `PARSER_VERSION` bump is.
    fn age_parser_version(&self) {
        self.conn
            .execute("UPDATE ingest_state SET parser_version = '3'", [])
            .unwrap();
    }

    /// Rewrite the source with different content (not an append).
    fn rewrite(&self) {
        std::fs::write(
            &self.path,
            line("u1", "user", "question again")
                + &line("u2", "assistant", "answer names the larch"),
        )
        .unwrap();
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[test]
fn a_filed_session_keeps_its_folder_across_reingest_and_a_parser_bump() {
    let archive = Archive::new();
    let session = archive.session_id().unwrap();
    let folder = lore_core::folders::create_folder(&archive.conn, "Kept").unwrap();
    lore_core::folders::set_session_folder(&archive.conn, &session, Some(&folder.id)).unwrap();
    let added_at: i64 = archive
        .conn
        .query_row("SELECT added_at FROM session_folder", [], |r| r.get(0))
        .unwrap();

    archive.age_parser_version();
    assert!(matches!(archive.ingest(), IngestOutcome::Ingested { .. }));
    assert_eq!(
        lore_core::folders::folder_of_session(&archive.conn, &session).unwrap(),
        Some(folder.id.clone()),
        "a parser bump must not unfile the session"
    );

    archive.rewrite();
    assert!(matches!(archive.ingest(), IngestOutcome::Ingested { .. }));
    assert_eq!(
        lore_core::folders::folder_of_session(&archive.conn, &session).unwrap(),
        Some(folder.id),
        "a rewritten source must not unfile the session"
    );
    let kept_added_at: i64 = archive
        .conn
        .query_row("SELECT added_at FROM session_folder", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept_added_at, added_at);
    assert!(archive.path().exists());
}

#[test]
fn a_forgotten_session_stays_gone_after_a_parser_bump() {
    let archive = Archive::new();
    let session = archive.session_id().unwrap();
    lore_core::forget::forget_session(&archive.conn, &archive.blobs, &session).unwrap();
    assert!(
        archive.path().exists(),
        "forget never deletes the agent log"
    );

    archive.age_parser_version();
    assert!(matches!(archive.ingest(), IngestOutcome::Skipped));
    assert_eq!(archive.session_id(), None, "a forgotten session came back");
    assert!(matches!(archive.ingest(), IngestOutcome::Skipped));
    assert_eq!(archive.session_id(), None);
}
