//! Ingest reads a source's state, parses the file, and only then takes the
//! write lock to store what it parsed. A forget that commits in between, from
//! the UI's connection while the worker parses, must win: the ingest in flight
//! stores nothing and the source stays forgotten.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::{
    AgentAdapter, AgentId, AgentMetadata, ArchivedSession, Capabilities, Detection, DiscoveryRoots,
    SessionRef,
};
use lore_core::ingest::{ingest_file, IngestOutcome};
use lore_core::model::{ParseCheckpoint, ParsedAppend, ParsedSession};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

const SESSION: &str = "14141414-0000-4000-8000-000000000001";

type Forget = fn(&Connection, &BlobStore);

/// The Claude adapter, running `forget` on its own connection to the archive
/// the first time it parses, as the UI would while the worker parses.
struct ForgetsWhileParsing {
    inner: ClaudeCodeAdapter,
    pending: Mutex<Option<(PathBuf, Forget)>>,
}

impl ForgetsWhileParsing {
    fn new(archive_dir: &Path, forget: Forget) -> Self {
        Self {
            inner: ClaudeCodeAdapter::new(),
            pending: Mutex::new(Some((archive_dir.to_path_buf(), forget))),
        }
    }

    fn forget_now(&self) {
        if let Some((dir, forget)) = self.pending.lock().unwrap().take() {
            let conn = lore_core::storage::open(&dir.join("lore.db")).unwrap();
            forget(&conn, &BlobStore::open(dir.join("blobs")).unwrap());
        }
    }

    fn forgot(&self) -> bool {
        self.pending.lock().unwrap().is_none()
    }
}

impl AgentAdapter for ForgetsWhileParsing {
    fn id(&self) -> AgentId {
        self.inner.id()
    }
    fn metadata(&self) -> AgentMetadata {
        self.inner.metadata()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    fn detect_installation(&self, roots: &DiscoveryRoots) -> Detection {
        self.inner.detect_installation(roots)
    }
    fn roots(&self, overrides: &DiscoveryRoots) -> Vec<PathBuf> {
        self.inner.roots(overrides)
    }
    fn discover_sessions(&self, roots: &DiscoveryRoots) -> Vec<SessionRef> {
        self.inner.discover_sessions(roots)
    }
    fn parse_content(&self, content: &str, fallback_dedupe: &str) -> ParsedSession {
        self.forget_now();
        self.inner.parse_content(content, fallback_dedupe)
    }
    fn parse_checkpointed(
        &self,
        content: &str,
        fallback_dedupe: &str,
    ) -> (ParsedSession, Option<ParseCheckpoint>) {
        self.forget_now();
        self.inner.parse_checkpointed(content, fallback_dedupe)
    }
    fn parse_append(
        &self,
        from: &ParseCheckpoint,
        tail: &str,
        fallback_dedupe: &str,
        archived: &dyn ArchivedSession,
    ) -> Option<ParsedAppend> {
        self.forget_now();
        self.inner
            .parse_append(from, tail, fallback_dedupe, archived)
    }
}

fn line(uuid: &str, role: &str, text: &str) -> String {
    format!(
        r#"{{"type":"{role}","uuid":"{uuid}","sessionId":"{SESSION}","timestamp":"2026-10-31T18:00:00.000Z","cwd":"/nonexistent/lore-test","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    ) + "\n"
}

fn forget_the_session(conn: &Connection, blobs: &BlobStore) {
    let id: String = conn
        .query_row("SELECT id FROM agent_session", [], |r| r.get(0))
        .unwrap();
    lore_core::forget::forget_session(conn, blobs, &id).unwrap();
}

fn forget_everything(conn: &Connection, blobs: &BlobStore) {
    lore_core::forget::forget_all(conn, blobs).unwrap();
}

/// Archive one transcript, change it with `change`, then ingest it again while
/// `forget` commits mid-parse. Returns the outcome, the sessions left, and the
/// source's state.
fn ingest_racing(change: fn(&Path), forget: Forget) -> (IngestOutcome, i64, String) {
    let dir = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open(&dir.path().join("lore.db")).unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let path = dir.path().join(format!("{SESSION}.jsonl"));
    std::fs::write(
        &path,
        line("u1", "user", "question") + &line("u2", "assistant", "answer names the larch"),
    )
    .unwrap();
    let first = ingest_file(&conn, &ClaudeCodeAdapter::new(), &path, &blobs).unwrap();
    assert!(matches!(first, IngestOutcome::Ingested { .. }));

    change(&path);
    let racing = ForgetsWhileParsing::new(dir.path(), forget);
    let outcome = ingest_file(&conn, &racing, &path, &blobs).unwrap();
    assert!(racing.forgot(), "the forget ran during the parse");

    let sessions = conn
        .query_row("SELECT count(*) FROM agent_session", [], |r| r.get(0))
        .unwrap();
    let state = conn
        .query_row("SELECT state FROM ingest_state", [], |r| r.get(0))
        .unwrap();
    (outcome, sessions, state)
}

fn append(path: &Path) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(line("u3", "user", "one more line").as_bytes())
        .unwrap();
}

fn rewrite(path: &Path) {
    std::fs::write(
        path,
        line("u1", "user", "question again") + &line("u2", "assistant", "answer names the larch"),
    )
    .unwrap();
}

#[test]
fn a_forget_during_a_reparse_is_not_undone_by_it() {
    for forget in [forget_the_session as Forget, forget_everything] {
        let (outcome, sessions, state) = ingest_racing(rewrite, forget);
        assert_eq!(sessions, 0, "the ingest in flight restored the session");
        assert_eq!(state, "forgotten");
        assert!(matches!(outcome, IngestOutcome::Skipped));
    }
}

#[test]
fn a_forget_during_an_append_is_not_undone_by_it() {
    for forget in [forget_the_session as Forget, forget_everything] {
        let (outcome, sessions, state) = ingest_racing(append, forget);
        assert_eq!(sessions, 0, "the ingest in flight restored the session");
        assert_eq!(state, "forgotten");
        assert!(matches!(outcome, IngestOutcome::Skipped));
    }
}
