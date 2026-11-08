//! Ingest links every source it parses to a session. An `ok` source that no
//! session links to is therefore read as one an older Lore forgot before
//! forget marked sources (`SourceRow::forgotten`). That reading holds only
//! while no live path leaves such a source: after every ingest path, the only
//! unlinked sources are the ones the user forgot, and they are `forgotten`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::path::PathBuf;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::ingest::{ingest_file, ChangeClass, IngestOutcome};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

const KEPT: &str = "13131313-0000-4000-8000-000000000001";
const FORGOTTEN: &str = "13131313-0000-4000-8000-000000000002";
const FIRST_KEY: &str = "13131313-0000-4000-8000-000000000003";
const SECOND_KEY: &str = "13131313-0000-4000-8000-000000000004";
/// Not a session id, so this transcript is keyed by its first line's
/// `sessionId` and a rewrite can move it to another session.
const MOVED: &str = "moved";

fn line(session: &str, uuid: &str, role: &str, text: &str) -> String {
    format!(
        r#"{{"type":"{role}","uuid":"{uuid}","sessionId":"{session}","timestamp":"2026-10-31T18:00:00.000Z","cwd":"/nonexistent/lore-test","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    ) + "\n"
}

fn change(outcome: IngestOutcome) -> Option<ChangeClass> {
    match outcome {
        IngestOutcome::Ingested { change, .. } => Some(change),
        IngestOutcome::Skipped => None,
    }
}

struct Archive {
    conn: Connection,
    blobs: BlobStore,
    _blob_dir: tempfile::TempDir,
    dir: tempfile::TempDir,
}

impl Archive {
    fn new() -> Self {
        let blob_dir = tempfile::tempdir().unwrap();
        Self {
            conn: lore_core::storage::open_in_memory().unwrap(),
            blobs: BlobStore::open(blob_dir.path()).unwrap(),
            _blob_dir: blob_dir,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!("{name}.jsonl"))
    }

    fn write(&self, name: &str, session: &str, answer: &str) {
        std::fs::write(
            self.path(name),
            line(session, "u1", "user", "question") + &line(session, "u2", "assistant", answer),
        )
        .unwrap();
    }

    fn append(&self, name: &str, session: &str, answer: &str) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(self.path(name))
            .unwrap();
        file.write_all(line(session, "u3", "assistant", answer).as_bytes())
            .unwrap();
    }

    fn ingest(&self, name: &str) -> Option<ChangeClass> {
        change(
            ingest_file(
                &self.conn,
                &ClaudeCodeAdapter::new(),
                &self.path(name),
                &self.blobs,
            )
            .unwrap(),
        )
    }

    fn ingest_all(&self) -> Vec<Option<ChangeClass>> {
        [KEPT, FORGOTTEN, MOVED]
            .iter()
            .map(|name| self.ingest(name))
            .collect()
    }

    fn age_parser_version(&self) {
        self.conn
            .execute("UPDATE ingest_state SET parser_version = 'old'", [])
            .unwrap();
    }

    fn session(&self, native_session_id: &str) -> String {
        self.conn
            .query_row(
                "SELECT id FROM agent_session WHERE native_session_id = ?1",
                [native_session_id],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn native_ids(&self) -> Vec<String> {
        let mut stmt = self
            .conn
            .prepare("SELECT native_session_id FROM agent_session ORDER BY native_session_id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap()
    }

    /// (file name, ingest state) of every source no session links to.
    fn unlinked(&self) -> Vec<(String, String)> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT sa.current_path, COALESCE(ist.state, '') FROM source_artifact sa
                 LEFT JOIN ingest_state ist ON ist.source_artifact_id = sa.id
                 WHERE NOT EXISTS
                     (SELECT 1 FROM session_source ss WHERE ss.source_artifact_id = sa.id)
                 ORDER BY sa.current_path",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<(String, String)>>>()
            .unwrap()
            .into_iter()
            .map(|(path, state)| {
                let name = PathBuf::from(path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                (name, state)
            })
            .collect()
    }
}

fn forgotten_source() -> Vec<(String, String)> {
    vec![(format!("{FORGOTTEN}.jsonl"), "forgotten".to_string())]
}

#[test]
fn no_ingest_path_leaves_an_ok_source_without_a_session() {
    use ChangeClass::{Appended, New, Rewritten, Unchanged};
    let archive = Archive::new();
    archive.write(KEPT, KEPT, "kept answer");
    archive.write(FORGOTTEN, FORGOTTEN, "forgotten answer");
    archive.write(MOVED, FIRST_KEY, "first answer");

    assert_eq!(archive.ingest_all(), vec![Some(New); 3]);
    assert_eq!(archive.native_ids(), vec![KEPT, FORGOTTEN, FIRST_KEY]);
    assert_eq!(archive.unlinked(), vec![], "after a fresh ingest");

    archive.append(KEPT, KEPT, "kept answer grows");
    assert_eq!(archive.ingest(KEPT), Some(Appended));
    assert_eq!(archive.unlinked(), vec![], "after an append");

    archive.write(MOVED, SECOND_KEY, "the moved answer");
    assert_eq!(archive.ingest(MOVED), Some(Rewritten));
    assert_eq!(archive.native_ids(), vec![KEPT, FORGOTTEN, SECOND_KEY]);
    assert_eq!(archive.unlinked(), vec![], "after a re-key");

    archive.age_parser_version();
    assert_eq!(archive.ingest_all(), vec![Some(Unchanged); 3]);
    assert_eq!(archive.unlinked(), vec![], "after a parser bump");

    let session = archive.session(FORGOTTEN);
    lore_core::forget::forget_session(&archive.conn, &archive.blobs, &session).unwrap();
    assert_eq!(archive.unlinked(), forgotten_source(), "after a forget");

    archive.age_parser_version();
    assert_eq!(
        archive.ingest_all(),
        vec![Some(Unchanged), None, Some(Unchanged)]
    );
    assert_eq!(archive.native_ids(), vec![KEPT, SECOND_KEY]);
    assert_eq!(
        archive.unlinked(),
        forgotten_source(),
        "after a parser bump that follows a forget"
    );

    lore_core::forget::forget_all(&archive.conn, &archive.blobs).unwrap();
    assert_eq!(archive.native_ids(), Vec::<String>::new());
    let all_forgotten: Vec<(String, String)> = [KEPT, FORGOTTEN, MOVED]
        .iter()
        .map(|name| (format!("{name}.jsonl"), "forgotten".to_string()))
        .collect();
    assert_eq!(
        archive.unlinked(),
        all_forgotten,
        "after forgetting everything"
    );

    assert_eq!(archive.ingest_all(), vec![None; 3]);
    assert_eq!(archive.native_ids(), Vec::<String>::new());
    assert_eq!(
        archive.unlinked(),
        all_forgotten,
        "after a scan that follows forgetting everything"
    );
}
