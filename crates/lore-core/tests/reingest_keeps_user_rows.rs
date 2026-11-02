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

const NO_REPO: &str = "/nonexistent/lore-test";

fn line_at(cwd: &str, uuid: &str, role: &str, text: &str) -> String {
    format!(
        r#"{{"type":"{role}","uuid":"{uuid}","sessionId":"{SESSION}","timestamp":"2026-10-31T18:00:00.000Z","cwd":"{cwd}","gitBranch":"main","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    ) + "\n"
}

fn line(uuid: &str, role: &str, text: &str) -> String {
    line_at(NO_REPO, uuid, role, text)
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
        Self::at(NO_REPO)
    }

    /// A one-session archive whose transcript ran in `cwd`.
    fn at(cwd: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{SESSION}.jsonl"));
        std::fs::write(
            &path,
            line_at(cwd, "u1", "user", "question")
                + &line_at(cwd, "u2", "assistant", "answer names the larch"),
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

/// An archive written before forget marked sources: the source is still `ok`
/// but supports no session.
#[test]
fn a_session_forgotten_by_an_older_lore_stays_gone_after_a_parser_bump() {
    let archive = Archive::new();
    let session = archive.session_id().unwrap();
    lore_core::forget::forget_session(&archive.conn, &archive.blobs, &session).unwrap();
    archive
        .conn
        .execute("UPDATE ingest_state SET state = 'ok'", [])
        .unwrap();

    archive.age_parser_version();
    assert!(matches!(archive.ingest(), IngestOutcome::Skipped));
    assert_eq!(archive.session_id(), None, "a forgotten session came back");
}

fn git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git must be installed");
    assert!(output.status.success(), "git {args:?} failed");
}

/// (source, observed_at) rows.
type Evidence = Vec<(String, i64)>;

/// (source, observed_at) of every git observation, and of every search_git row.
fn git_evidence(conn: &Connection) -> (Evidence, Evidence) {
    let rows = |sql: &str| {
        let mut stmt = conn.prepare(sql).unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<(String, i64)>>>()
            .unwrap()
    };
    (
        rows("SELECT source, observed_at FROM git_observation ORDER BY source, id"),
        rows("SELECT source_class, observed_at FROM search_git ORDER BY source_class, observed_at"),
    )
}

#[test]
fn git_evidence_keeps_when_it_was_first_observed_across_a_parser_bump() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-b", "main"]);
    std::fs::write(repo.path().join("README.md"), "hello\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "initial"]);
    let archive = Archive::at(&repo.path().to_string_lossy());
    let session = archive.session_id().unwrap();
    assert!(lore_core::enrich::enrich_session(&archive.conn, &session).unwrap() > 0);

    // Date the evidence in the past so a rewrite to now is visible, and add a
    // re-verification the way the reverify job records one.
    archive
        .conn
        .execute_batch(
            "UPDATE git_observation SET observed_at = 1000;
             INSERT INTO git_observation
                (id, session_id, segment_id, source, observed_at, temporal_confidence,
                 branch, commit_sha, commit_exists)
             SELECT 'gr_kept', session_id, segment_id, 'lore_reverified', 2000,
                    'current_only', 'main', 'abc123', 0
             FROM git_observation WHERE source = 'agent_recorded';
             UPDATE search_git SET observed_at = 1000;",
        )
        .unwrap();
    let kept = archive.conn.query_row(
        "SELECT count(*) FROM git_observation WHERE source = 'lore_captured'",
        [],
        |r| r.get::<_, i64>(0),
    );
    assert_eq!(kept.unwrap(), 1, "enrichment records lore_captured");
    let before = git_evidence(&archive.conn).0;

    archive.age_parser_version();
    assert!(matches!(archive.ingest(), IngestOutcome::Ingested { .. }));
    lore_core::enrich::enrich_session(&archive.conn, &session).unwrap();
    let (after, search_git) = git_evidence(&archive.conn);
    assert_eq!(after, before, "re-parsing rewrote or dropped git evidence");
    assert!(
        search_git.iter().all(|(_, at)| *at <= 2000),
        "search_git must project the kept observed_at: {search_git:?}"
    );
    assert!(search_git
        .iter()
        .any(|(class, _)| class == "lore_reverified"));
}
