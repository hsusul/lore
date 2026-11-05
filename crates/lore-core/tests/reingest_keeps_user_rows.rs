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

    /// Mark every source as parsed by an older parser, as an archive written
    /// before a `PARSER_VERSION` bump is. `'old'` is never a real version, so
    /// a bump never makes it current and the re-parse always happens.
    fn age_parser_version(&self) {
        assert_ne!(lore_core::ingest::PARSER_VERSION, "old");
        self.conn
            .execute("UPDATE ingest_state SET parser_version = 'old'", [])
            .unwrap();
    }

    /// Rewrite the source with different content (not an append).
    fn rewrite(&self) {
        self.rewrite_at(NO_REPO);
    }

    /// Rewrite the source with different content that ran in `cwd`.
    fn rewrite_at(&self, cwd: &str) {
        std::fs::write(
            &self.path,
            line_at(cwd, "u1", "user", "question again")
                + &line_at(cwd, "u2", "assistant", "answer names the larch"),
        )
        .unwrap();
    }

    /// Append one line, as an agent still writing the session does.
    fn append(&self, uuid: &str) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .unwrap();
        file.write_all(line_at(NO_REPO, uuid, "user", "one more line").as_bytes())
            .unwrap();
    }

    /// Sessions, messages, and search documents in the archive.
    fn derived_rows(&self) -> (i64, i64, i64) {
        let count = |table: &str| -> i64 {
            self.conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        (
            count("agent_session"),
            count("message"),
            count("search_document"),
        )
    }

    fn source_state(&self) -> String {
        self.conn
            .query_row("SELECT state FROM ingest_state", [], |r| r.get(0))
            .unwrap()
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

/// A forgotten source stays forgotten while it exists, even when the agent
/// keeps writing it: nothing it holds is ingested again.
#[test]
fn a_forgotten_session_stays_gone_after_its_log_changes() {
    let archive = Archive::new();
    let session = archive.session_id().unwrap();
    lore_core::forget::forget_session(&archive.conn, &archive.blobs, &session).unwrap();
    assert_eq!(archive.source_state(), "forgotten");

    archive.append("u3");
    let outcome = archive.ingest();
    assert_eq!(archive.derived_rows(), (0, 0, 0), "an append restored rows");
    assert_eq!(archive.session_id(), None);
    assert!(matches!(outcome, IngestOutcome::Skipped));
    assert_eq!(archive.source_state(), "forgotten");

    archive.append("u4");
    assert!(matches!(archive.ingest(), IngestOutcome::Skipped));
    archive.rewrite();
    assert!(matches!(archive.ingest(), IngestOutcome::Skipped));
    assert_eq!(archive.session_id(), None, "a rewrite restored the session");
    assert_eq!(archive.derived_rows(), (0, 0, 0));
    assert_eq!(archive.source_state(), "forgotten");
}

/// The same for a source an older Lore left `ok` with no session.
#[test]
fn a_session_forgotten_by_an_older_lore_stays_gone_after_its_log_changes() {
    let archive = Archive::new();
    let session = archive.session_id().unwrap();
    lore_core::forget::forget_session(&archive.conn, &archive.blobs, &session).unwrap();
    archive
        .conn
        .execute("UPDATE ingest_state SET state = 'ok'", [])
        .unwrap();

    archive.append("u3");
    let outcome = archive.ingest();
    assert_eq!(archive.derived_rows(), (0, 0, 0), "an append restored rows");
    assert_eq!(archive.session_id(), None);
    assert!(matches!(outcome, IngestOutcome::Skipped));

    archive.rewrite();
    assert!(matches!(archive.ingest(), IngestOutcome::Skipped));
    assert_eq!(archive.session_id(), None, "a rewrite restored the session");
    assert_eq!(archive.derived_rows(), (0, 0, 0));
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
        .env("GIT_AUTHOR_DATE", "2026-10-31T18:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-31T18:00:00Z")
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
    let observed = vec![
        ("agent_recorded".to_string(), 1000),
        ("lore_captured".to_string(), 1000),
        ("lore_reflog".to_string(), 1000),
        ("lore_reverified".to_string(), 2000),
    ];
    assert_eq!(git_evidence(&archive.conn).0, observed, "the seed took");

    archive.age_parser_version();
    assert!(matches!(archive.ingest(), IngestOutcome::Ingested { .. }));
    lore_core::enrich::enrich_session(&archive.conn, &session).unwrap();
    let (after, search_git) = git_evidence(&archive.conn);
    assert_eq!(
        after, observed,
        "re-parsing rewrote or dropped git evidence"
    );
    // `segment_link` rows are synthetic and always carry observed_at 0.
    let projected = vec![
        ("agent_recorded".to_string(), 1000),
        ("lore_captured".to_string(), 1000),
        ("lore_reflog".to_string(), 1000),
        ("lore_reverified".to_string(), 2000),
        ("segment_link".to_string(), 0),
    ];
    assert_eq!(
        search_git, projected,
        "search_git must project the kept observed_at"
    );
}

/// A repository on `branch` with one commit of `text`. The commit date is
/// fixed, so its sha is known.
fn repo_on(branch: &str, text: &str) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-b", branch]);
    std::fs::write(repo.path().join("README.md"), text).unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "initial"]);
    repo
}

/// (source, branch, commit) of the git evidence Lore recorded on segment 0.
type LoreEvidence = Vec<(String, Option<String>, Option<String>)>;

fn lore_evidence(conn: &Connection, session: &str) -> LoreEvidence {
    let mut stmt = conn
        .prepare(
            "SELECT g.source, g.branch, g.commit_sha FROM git_observation g
             JOIN session_segment seg ON seg.id = g.segment_id
             WHERE g.session_id = ?1 AND seg.seq_start = 0
               AND g.source IN ('lore_captured', 'lore_reverified')
             ORDER BY g.source, g.id",
        )
        .unwrap();
    stmt.query_map([session], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<LoreEvidence>>()
        .unwrap()
}

fn evidence(source: &str, branch: &str, commit: &str) -> (String, Option<String>, Option<String>) {
    (
        source.to_string(),
        Some(branch.to_string()),
        Some(commit.to_string()),
    )
}

#[test]
fn a_segment_whose_cwd_moves_to_another_repository_drops_the_old_repositorys_evidence() {
    const OLD_SHA: &str = "e09836f70174a380c9c02472de679a7807f4ed5f";
    const NEW_SHA: &str = "edbaf241d3277779b5f97fddc0512bc3bcd3dcb1";
    let old = repo_on("alpha", "old\n");
    let new = repo_on("beta", "new\n");
    let archive = Archive::at(&old.path().to_string_lossy());
    let session = archive.session_id().unwrap();
    assert!(lore_core::enrich::enrich_session(&archive.conn, &session).unwrap() > 0);
    // A re-verification of the old repository, the way the reverify job
    // records one.
    archive
        .conn
        .execute_batch(
            "INSERT INTO git_observation
                (id, session_id, segment_id, source, observed_at, temporal_confidence,
                 branch, commit_sha, commit_exists)
             SELECT 'gr_old', session_id, segment_id, 'lore_reverified', 2000,
                    'retrospective', branch, commit_sha, 1
             FROM git_observation WHERE source = 'lore_captured';",
        )
        .unwrap();
    assert_eq!(
        lore_evidence(&archive.conn, &session),
        vec![
            evidence("lore_captured", "alpha", OLD_SHA),
            evidence("lore_reverified", "alpha", OLD_SHA),
        ]
    );

    archive.rewrite_at(&new.path().to_string_lossy());
    assert!(matches!(archive.ingest(), IngestOutcome::Ingested { .. }));
    assert_eq!(
        lore_evidence(&archive.conn, &session),
        vec![],
        "the old repository's evidence came back on a segment that moved"
    );
    assert!(lore_core::enrich::enrich_session(&archive.conn, &session).unwrap() > 0);
    assert_eq!(
        lore_evidence(&archive.conn, &session),
        vec![evidence("lore_captured", "beta", NEW_SHA)]
    );
}
