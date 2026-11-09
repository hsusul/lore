//! Ingest reads a source's state, parses the file, and only then takes the
//! write lock to store what it parsed. A forget that commits in between, from
//! the UI's connection while the worker parses, must win: the ingest in flight
//! stores nothing, leaves none of the files it staged, and the source stays
//! forgotten.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
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
const OTHER: &str = "15151515-0000-4000-8000-000000000002";

type Forget = fn(&Connection, &BlobStore);

/// The Claude adapter, running `forget` on its own connection to the archive
/// the first time it parses, as the UI would while the worker parses.
struct ForgetsWhileParsing {
    inner: ClaudeCodeAdapter,
    pending: Mutex<Option<(PathBuf, Forget)>>,
    parses: AtomicUsize,
}

impl ForgetsWhileParsing {
    fn new(archive_dir: &Path, forget: Forget) -> Self {
        Self {
            inner: ClaudeCodeAdapter::new(),
            pending: Mutex::new(Some((archive_dir.to_path_buf(), forget))),
            parses: AtomicUsize::new(0),
        }
    }

    fn forget_now(&self) {
        self.parses.fetch_add(1, Ordering::SeqCst);
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

/// A user line carrying a screenshot, which ingest stages as a blob file.
fn screenshot(uuid: &str) -> String {
    let data = "QUJD".repeat(2048);
    format!(
        r#"{{"type":"user","uuid":"{uuid}","sessionId":"{SESSION}","timestamp":"2026-10-31T18:00:00.000Z","cwd":"/nonexistent/lore-test","message":{{"role":"user","content":[{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{data}"}}}},{{"type":"text","text":"see screenshot"}}]}}}}"#
    ) + "\n"
}

fn blob_files(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .map(|e| match e.path() {
                p if p.is_dir() => blob_files(&p),
                _ => 1,
            })
            .sum()
    })
}

fn forget_the_session(conn: &Connection, blobs: &BlobStore) {
    let id: String = conn
        .query_row(
            "SELECT id FROM agent_session WHERE native_session_id = ?1",
            [SESSION],
            |r| r.get(0),
        )
        .unwrap();
    lore_core::forget::forget_session(conn, blobs, &id).unwrap();
}

fn forget_everything(conn: &Connection, blobs: &BlobStore) {
    lore_core::forget::forget_all(conn, blobs).unwrap();
}

struct Raced {
    outcome: IngestOutcome,
    sessions: i64,
    state: String,
    blob_files: usize,
    parses: usize,
}

/// Archive one transcript, change it with `change`, then ingest it again while
/// `forget` commits mid-parse.
fn ingest_racing(change: fn(&Path), forget: Forget) -> Raced {
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
    Raced {
        outcome,
        sessions,
        state,
        blob_files: blob_files(blobs.root()),
        parses: racing.parses.load(Ordering::SeqCst),
    }
}

fn append(path: &Path) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(line("u3", "user", "one more line").as_bytes())
        .unwrap();
}

fn append_screenshot(path: &Path) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(screenshot("u3").as_bytes()).unwrap();
}

fn rewrite_with_screenshot(path: &Path) {
    std::fs::write(
        path,
        screenshot("u1") + &line("u2", "assistant", "answer names the larch"),
    )
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
        let raced = ingest_racing(rewrite, forget);
        assert_eq!(
            raced.sessions, 0,
            "the ingest in flight restored the session"
        );
        assert_eq!(raced.state, "forgotten");
        assert!(matches!(raced.outcome, IngestOutcome::Skipped));
    }
}

#[test]
fn a_forget_during_an_append_is_not_undone_by_it() {
    for forget in [forget_the_session as Forget, forget_everything] {
        let raced = ingest_racing(append, forget);
        assert_eq!(
            raced.sessions, 0,
            "the ingest in flight restored the session"
        );
        assert_eq!(raced.state, "forgotten");
        assert!(matches!(raced.outcome, IngestOutcome::Skipped));
    }
}

/// A log Lore had never read before the forget is new, not forgotten: it is
/// archived whole, and the forget still removes everything older.
#[test]
fn a_new_log_ingested_during_forget_everything_is_archived_whole() {
    let dir = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open(&dir.path().join("lore.db")).unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let old = dir.path().join(format!("{OTHER}.jsonl"));
    std::fs::write(&old, line("o1", "user", "older").replace(SESSION, OTHER)).unwrap();
    ingest_file(&conn, &ClaudeCodeAdapter::new(), &old, &blobs).unwrap();

    let new = dir.path().join(format!("{SESSION}.jsonl"));
    std::fs::write(
        &new,
        line("u1", "user", "question") + &line("u2", "assistant", "answer"),
    )
    .unwrap();
    let racing = ForgetsWhileParsing::new(dir.path(), forget_everything);
    let outcome = ingest_file(&conn, &racing, &new, &blobs).unwrap();
    assert!(racing.forgot(), "the forget ran during the parse");

    assert!(
        matches!(outcome, IngestOutcome::Ingested { .. }),
        "{outcome:?}"
    );
    let natives: Vec<String> = conn
        .prepare("SELECT native_session_id FROM agent_session")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(natives, [SESSION], "only the new log is archived");
    let messages: i64 = conn
        .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
        .unwrap();
    assert_eq!(messages, 2, "the new log is archived whole");
    let old_state: String = conn
        .query_row(
            "SELECT s.state FROM ingest_state s JOIN source_artifact a ON a.id = s.source_artifact_id
              WHERE a.current_path LIKE ?1",
            [format!("%{OTHER}.jsonl")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old_state, "forgotten");
}

#[test]
fn a_forget_during_an_ingest_leaves_none_of_its_files() {
    for change in [append_screenshot as fn(&Path), rewrite_with_screenshot] {
        for forget in [forget_the_session as Forget, forget_everything] {
            let raced = ingest_racing(change, forget);
            assert!(matches!(raced.outcome, IngestOutcome::Skipped));
            assert_eq!(raced.blob_files, 0, "a forgotten screenshot stayed on disk");
            assert_eq!(raced.parses, 1, "the forgotten source was parsed again");
        }
    }
}

#[test]
fn a_forget_during_an_ingest_keeps_a_file_another_session_holds() {
    let dir = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open(&dir.path().join("lore.db")).unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let other = dir.path().join(format!("{OTHER}.jsonl"));
    std::fs::write(&other, screenshot("o1").replace(SESSION, OTHER)).unwrap();
    ingest_file(&conn, &ClaudeCodeAdapter::new(), &other, &blobs).unwrap();
    let path = dir.path().join(format!("{SESSION}.jsonl"));
    std::fs::write(&path, line("u1", "user", "question")).unwrap();
    ingest_file(&conn, &ClaudeCodeAdapter::new(), &path, &blobs).unwrap();

    append_screenshot(&path);
    let racing = ForgetsWhileParsing::new(dir.path(), forget_the_session);
    let outcome = ingest_file(&conn, &racing, &path, &blobs).unwrap();
    assert!(racing.forgot(), "the forget ran during the parse");
    assert!(matches!(outcome, IngestOutcome::Skipped));

    let relpath: String = conn
        .query_row("SELECT storage_relpath FROM blob", [], |r| r.get(0))
        .unwrap();
    assert!(
        blobs.read(&relpath).is_ok(),
        "the other session lost its screenshot"
    );
}
