//! M4 acceptance: git enrichment resolves repository/worktree identity from a
//! segment's cwd, records a `lore_captured` observation separate from the
//! agent-recorded one, groups linked worktrees under one repository, and leaves
//! a non-git cwd unlinked.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::enrich::enrich_session;
use lore_core::ingest::persist_session;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
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
        .expect("git must be installed to run M4 enrich tests");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "initial"]);
}

/// A one-message Claude session whose recorded cwd is `cwd`.
fn persist_session_at(conn: &Connection, blobs: &BlobStore, dedupe: &str, cwd: &Path) -> String {
    let content = format!(
        "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"{dedupe}\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n",
        cwd.to_string_lossy()
    );
    let parsed = ClaudeCodeAdapter::new().parse_str(&content, dedupe);
    persist_session(conn, "claude-code", "Claude Code", &parsed, blobs).unwrap()
}

fn blobs() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    (dir, store)
}

#[test]
fn enrich_resolves_repository_and_records_lore_captured_observation() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid = persist_session_at(&conn, &store, "enrich", repo.path());
    let linked = enrich_session(&conn, &sid).unwrap();
    assert_eq!(linked, 1);

    // The segment is now linked with high confidence.
    let (repo_id, confidence): (String, String) = conn
        .query_row(
            "SELECT repository_id, resolution_confidence FROM session_segment
             WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(confidence, "high");

    let (rep_conf, is_primary): (String, i64) = conn
        .query_row(
            "SELECT r.identity_confidence, w.is_primary
             FROM repository r JOIN worktree w ON w.repository_id = r.id
             WHERE r.id = ?1",
            [&repo_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rep_conf, "high");
    assert_eq!(is_primary, 1, "the main worktree is primary");

    // A git_common_dir evidence row backs the identity.
    let ev: i64 = conn
        .query_row(
            "SELECT count(*) FROM repository_identity_evidence
             WHERE repository_id = ?1 AND kind = 'git_common_dir'",
            [&repo_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ev, 1);

    // Both provenances coexist and are separate: agent_recorded (branch) and
    // lore_captured (current state, dirty=false, real commit).
    let (agent, captured): (i64, i64) = conn
        .query_row(
            "SELECT
                (SELECT count(*) FROM git_observation WHERE session_id=?1 AND source='agent_recorded'),
                (SELECT count(*) FROM git_observation WHERE session_id=?1 AND source='lore_captured')",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(agent, 1);
    assert_eq!(captured, 1);

    let (tconf, branch, is_dirty, sha_len): (String, String, i64, usize) = conn
        .query_row(
            "SELECT temporal_confidence, branch, is_dirty, length(commit_sha)
             FROM git_observation WHERE session_id=?1 AND source='lore_captured'",
            [&sid],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, i64>(3)? as usize,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        tconf, "current_only",
        "captured state is never session-time"
    );
    assert_eq!(branch, "main");
    assert_eq!(is_dirty, 0);
    assert_eq!(sha_len, 40);
}

#[test]
fn linked_worktrees_group_under_one_repository() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main");
    std::fs::create_dir(&main).unwrap();
    init_repo(&main);
    let linked = root.path().join("feature-wt");
    git(
        &main,
        &["worktree", "add", linked.to_str().unwrap(), "-b", "feature"],
    );

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let main_sid = persist_session_at(&conn, &store, "main-sess", &main);
    let linked_sid = persist_session_at(&conn, &store, "linked-sess", &linked);
    enrich_session(&conn, &main_sid).unwrap();
    enrich_session(&conn, &linked_sid).unwrap();

    let repo_of = |sid: &str| -> String {
        conn.query_row(
            "SELECT repository_id FROM session_segment WHERE session_id = ?1",
            [sid],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        repo_of(&main_sid),
        repo_of(&linked_sid),
        "linked worktrees resolve to one repository identity"
    );

    // One repository, two distinct worktrees.
    let repos: i64 = conn
        .query_row("SELECT count(*) FROM repository", [], |r| r.get(0))
        .unwrap();
    let worktrees: i64 = conn
        .query_row("SELECT count(*) FROM worktree", [], |r| r.get(0))
        .unwrap();
    assert_eq!(repos, 1);
    assert_eq!(worktrees, 2);
}

/// Clone `src` to `dst` and point its origin at `remote_url`, so distinct local
/// clones share a root commit but carry a chosen (forge-style) remote.
fn clone_with_remote(root: &Path, src: &Path, dst: &Path, remote_url: &str) {
    git(
        root,
        &["clone", src.to_str().unwrap(), dst.to_str().unwrap()],
    );
    git(dst, &["remote", "set-url", "origin", remote_url]);
}

fn repo_of(conn: &Connection, sid: &str) -> String {
    conn.query_row(
        "SELECT repository_id FROM session_segment WHERE session_id = ?1",
        [sid],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn clones_of_one_upstream_merge_into_one_repository() {
    let root = tempfile::tempdir().unwrap();
    let upstream = root.path().join("upstream");
    std::fs::create_dir(&upstream).unwrap();
    init_repo(&upstream);

    // Two clones share the root commit and the same upstream remote URL.
    let a = root.path().join("a");
    let b = root.path().join("b");
    let url = "https://github.com/org/repo.git";
    clone_with_remote(root.path(), &upstream, &a, url);
    clone_with_remote(root.path(), &upstream, &b, url);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let a_sid = persist_session_at(&conn, &store, "a", &a);
    let b_sid = persist_session_at(&conn, &store, "b", &b);
    enrich_session(&conn, &a_sid).unwrap();
    enrich_session(&conn, &b_sid).unwrap();

    assert_eq!(
        repo_of(&conn, &a_sid),
        repo_of(&conn, &b_sid),
        "same remote + same root ⇒ one repository (clones merge)"
    );
    let repos: i64 = conn
        .query_row("SELECT count(*) FROM repository", [], |r| r.get(0))
        .unwrap();
    let worktrees: i64 = conn
        .query_row("SELECT count(*) FROM worktree", [], |r| r.get(0))
        .unwrap();
    assert_eq!(repos, 1);
    assert_eq!(worktrees, 2, "each clone is a distinct worktree");

    let confidence: String = conn
        .query_row("SELECT identity_confidence FROM repository", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(confidence, "high", "remote + root is high confidence");
}

#[test]
fn fork_sharing_a_root_is_not_merged_with_upstream() {
    let root = tempfile::tempdir().unwrap();
    let upstream = root.path().join("upstream");
    std::fs::create_dir(&upstream).unwrap();
    init_repo(&upstream);

    // Same root commit, but different remotes (upstream vs a fork).
    let up_clone = root.path().join("up");
    let fork = root.path().join("fork");
    clone_with_remote(
        root.path(),
        &upstream,
        &up_clone,
        "https://github.com/org/repo.git",
    );
    clone_with_remote(
        root.path(),
        &upstream,
        &fork,
        "https://github.com/someone-else/repo.git",
    );

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let up_sid = persist_session_at(&conn, &store, "up", &up_clone);
    let fork_sid = persist_session_at(&conn, &store, "fork", &fork);
    enrich_session(&conn, &up_sid).unwrap();
    enrich_session(&conn, &fork_sid).unwrap();

    assert_ne!(
        repo_of(&conn, &up_sid),
        repo_of(&conn, &fork_sid),
        "a fork shares the root but has a different remote ⇒ stays separate"
    );
    let repos: i64 = conn
        .query_row("SELECT count(*) FROM repository", [], |r| r.get(0))
        .unwrap();
    assert_eq!(repos, 2, "fork and upstream are distinct repositories");
}

#[test]
fn list_repositories_reports_enriched_repositories() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid = persist_session_at(&conn, &store, "repolist", repo.path());
    enrich_session(&conn, &sid).unwrap();

    let repos = lore_core::query::list_repositories(&conn).unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].identity_confidence, "high");
    assert_eq!(repos[0].session_count, 1);
    assert_eq!(repos[0].worktree_count, 1);
    assert!(!repos[0].is_missing);
    assert!(!repos[0].display_name.is_empty());

    // The session is reachable by filtering on its repository.
    let repo_sessions =
        lore_core::query::list_repository_sessions(&conn, &repos[0].id, 50).unwrap();
    assert_eq!(repo_sessions.len(), 1);
    assert_eq!(repo_sessions[0].id, sid);
    // A different repository id matches nothing.
    assert!(
        lore_core::query::list_repository_sessions(&conn, "nope", 50)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn non_git_cwd_is_left_unlinked() {
    let plain = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid = persist_session_at(&conn, &store, "plain", plain.path());
    assert_eq!(enrich_session(&conn, &sid).unwrap(), 0);

    let repo_id: Option<String> = conn
        .query_row(
            "SELECT repository_id FROM session_segment WHERE session_id = ?1",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        repo_id.is_none(),
        "a non-git cwd stays under 'No repository'"
    );
    let repos: i64 = conn
        .query_row("SELECT count(*) FROM repository", [], |r| r.get(0))
        .unwrap();
    assert_eq!(repos, 0);
}

#[test]
fn pipeline_enriches_automatically_on_ingest() {
    use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
    use lore_core::discovery::DiscoveryConfig;
    use lore_core::pipeline::{NullSink, Pipeline};

    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());

    // A discoverable Claude session file whose recorded cwd is the repo.
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("projects");
    let project = projects.join("encoded");
    std::fs::create_dir_all(&project).unwrap();
    let content = format!(
        "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"pipe\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n",
        repo.path().to_string_lossy()
    );
    std::fs::write(project.join("s.jsonl"), content).unwrap();

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();
    let registry = AdapterRegistry::v0();
    let mut config = DiscoveryConfig::new();
    config.set_roots("claude-code", DiscoveryRoots::new(vec![projects]));
    // Isolate Codex to an empty root so discovery never touches real ~/.codex.
    config.set_roots(
        "codex",
        DiscoveryRoots::new(vec![home.path().join("codex-empty")]),
    );
    let pipeline = Pipeline::new(&conn, &registry, &store, &config, 64);

    pipeline.enqueue_scan(&NullSink).unwrap();
    let summary = pipeline.drain(&NullSink, 10).unwrap();
    assert_eq!(summary.ingested, 1);
    assert_eq!(summary.enriched, 1, "ingest triggers enrichment");
    assert_eq!(summary.enrich_failed, 0);

    let repos: i64 = conn
        .query_row("SELECT count(*) FROM repository", [], |r| r.get(0))
        .unwrap();
    assert_eq!(repos, 1);
}

#[test]
fn enrich_is_idempotent() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid = persist_session_at(&conn, &store, "idem", repo.path());
    enrich_session(&conn, &sid).unwrap();
    // Second pass finds the segment already linked → no new work, no duplicates.
    assert_eq!(enrich_session(&conn, &sid).unwrap(), 0);

    for (table, expected) in [("repository", 1), ("worktree", 1)] {
        let n: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, expected, "{table} must not duplicate");
    }
    let captured: i64 = conn
        .query_row(
            "SELECT count(*) FROM git_observation WHERE source='lore_captured'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(captured, 1);
}

#[test]
fn multi_segment_session_in_same_repo_resolves_to_single_repository() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let sub = repo.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    // Two messages in different directories of the same repository (two segments).
    let content = format!(
        "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"multi-seg\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n\
         {{\"type\":\"user\",\"uuid\":\"u2\",\"sessionId\":\"multi-seg\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"in sub\"}}}}\n",
        repo.path().to_string_lossy(),
        sub.to_string_lossy()
    );
    let parsed = ClaudeCodeAdapter::new().parse_str(&content, "multi-seg");
    assert_eq!(parsed.segments.len(), 2);

    let sid = persist_session(&conn, "claude-code", "Claude Code", &parsed, &store).unwrap();
    let linked = enrich_session(&conn, &sid).unwrap();
    assert_eq!(linked, 2, "both segments must be linked");

    // Both segments must link to the EXACT same repository row.
    let repo_ids: Vec<String> = conn
        .prepare(
            "SELECT repository_id FROM session_segment WHERE session_id = ?1 ORDER BY seq_start",
        )
        .unwrap()
        .query_map([&sid], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(repo_ids.len(), 2);
    assert_eq!(repo_ids[0], repo_ids[1]);

    let total_repos: i64 = conn
        .query_row("SELECT count(*) FROM repository", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total_repos, 1, "exactly one repository should exist");
}

#[test]
fn user_can_relink_segment_to_different_repository() {
    let repo_a = tempfile::tempdir().unwrap();
    init_repo(repo_a.path());
    let repo_b = tempfile::tempdir().unwrap();
    init_repo(repo_b.path());

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid_a = persist_session_at(&conn, &store, "sess-a", repo_a.path());
    let sid_b = persist_session_at(&conn, &store, "sess-b", repo_b.path());
    enrich_session(&conn, &sid_a).unwrap();
    enrich_session(&conn, &sid_b).unwrap();

    let repo_a_id = repo_of(&conn, &sid_a);
    let repo_b_id = repo_of(&conn, &sid_b);
    assert_ne!(repo_a_id, repo_b_id);

    let segment_a_id: String = conn
        .query_row(
            "SELECT id FROM session_segment WHERE session_id = ?1",
            [&sid_a],
            |r| r.get(0),
        )
        .unwrap();

    // Relink segment from repo_a to repo_b
    lore_core::enrich::relink_segment_repository(&conn, &segment_a_id, &repo_b_id).unwrap();

    let new_repo_id: String = conn
        .query_row(
            "SELECT repository_id FROM session_segment WHERE id = ?1",
            [&segment_a_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_repo_id, repo_b_id);
}

#[test]
fn enrich_handles_detached_head_repository() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    // Create a second commit and detach HEAD
    std::fs::write(repo.path().join("second.txt"), "second\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "second commit"]);
    git(repo.path(), &["checkout", "--detach", "HEAD"]);

    let head_sha = {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid = persist_session_at(&conn, &store, "detached-sess", repo.path());
    let enriched = enrich_session(&conn, &sid).unwrap();
    assert_eq!(enriched, 1);

    // Segment is linked to repository and worktree
    let (repo_id, wt_id, confidence): (String, String, String) = conn
        .query_row(
            "SELECT repository_id, worktree_id, resolution_confidence FROM session_segment WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(confidence, "high");

    // Worktree branch_hint is NULL for detached HEAD
    let branch_hint: Option<String> = conn
        .query_row(
            "SELECT branch_hint FROM worktree WHERE id = ?1",
            [&wt_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        branch_hint, None,
        "branch_hint is NULL when HEAD is detached"
    );

    struct ObsRow {
        branch: Option<String>,
        commit_sha: Option<String>,
        commit_subject: Option<String>,
        ahead: Option<i64>,
        behind: Option<i64>,
        is_dirty: Option<i64>,
    }

    let obs: ObsRow = conn
        .query_row(
            "SELECT branch, commit_sha, commit_subject, ahead, behind, is_dirty
             FROM git_observation WHERE session_id = ?1 AND source = 'lore_captured'",
            [&sid],
            |r| {
                Ok(ObsRow {
                    branch: r.get(0)?,
                    commit_sha: r.get(1)?,
                    commit_subject: r.get(2)?,
                    ahead: r.get(3)?,
                    behind: r.get(4)?,
                    is_dirty: r.get(5)?,
                })
            },
        )
        .unwrap();

    assert_eq!(
        obs.branch, None,
        "detached HEAD observation has branch NULL"
    );
    assert_eq!(obs.commit_sha.as_deref(), Some(head_sha.as_str()));
    assert_eq!(obs.commit_subject.as_deref(), Some("second commit"));
    assert_eq!(obs.ahead, None, "ahead is NULL for detached HEAD");
    assert_eq!(obs.behind, None, "behind is NULL for detached HEAD");
    assert_eq!(obs.is_dirty, Some(0));

    // Search projection is updated with the observation
    let projected_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM search_git WHERE session_id = ?1 AND repository_id = ?2",
            [&sid, &repo_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(projected_count >= 1);
}

#[test]
fn enrich_handles_empty_commits() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-b", "main"]);
    // Create an empty commit
    git(
        repo.path(),
        &["commit", "--allow-empty", "-m", "empty initial commit"],
    );

    let head_sha = {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid = persist_session_at(&conn, &store, "empty-commit-sess", repo.path());
    let enriched = enrich_session(&conn, &sid).unwrap();
    assert_eq!(enriched, 1);

    let (branch, commit_sha, commit_subject, is_dirty): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
    ) = conn
        .query_row(
            "SELECT branch, commit_sha, commit_subject, is_dirty
             FROM git_observation WHERE session_id = ?1 AND source = 'lore_captured'",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();

    assert_eq!(branch.as_deref(), Some("main"));
    assert_eq!(commit_sha.as_deref(), Some(head_sha.as_str()));
    assert_eq!(commit_subject.as_deref(), Some("empty initial commit"));
    assert_eq!(is_dirty, Some(0));

    // Identity evidence contains root commit
    let (repo_id,): (String,) = conn
        .query_row(
            "SELECT repository_id FROM session_segment WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?,)),
        )
        .unwrap();

    let root_evidence_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM repository_identity_evidence WHERE repository_id = ?1 AND kind = 'root_set'",
            [&repo_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(root_evidence_count, 1);
}

#[test]
fn enrich_handles_unborn_repository_with_zero_commits() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-b", "main"]);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, store) = blobs();

    let sid = persist_session_at(&conn, &store, "unborn-sess", repo.path());
    let enriched = enrich_session(&conn, &sid).unwrap();
    assert_eq!(enriched, 1);

    // Segment linked
    let (repo_id, wt_id, confidence): (String, String, String) = conn
        .query_row(
            "SELECT repository_id, worktree_id, resolution_confidence FROM session_segment WHERE session_id = ?1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(confidence, "high");

    // Worktree has branch_hint = main
    let branch_hint: Option<String> = conn
        .query_row(
            "SELECT branch_hint FROM worktree WHERE id = ?1",
            [&wt_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(branch_hint.as_deref(), Some("main"));

    // Observation has branch = main, commit_sha = NULL, commit_subject = NULL
    let (branch, commit_sha, commit_subject): (Option<String>, Option<String>, Option<String>) =
        conn.query_row(
            "SELECT branch, commit_sha, commit_subject
             FROM git_observation WHERE session_id = ?1 AND source = 'lore_captured'",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    assert_eq!(branch.as_deref(), Some("main"));
    assert_eq!(commit_sha, None, "unborn repository has no head commit sha");
    assert_eq!(
        commit_subject, None,
        "unborn repository has no commit subject"
    );

    // Root commit evidence is not written for unborn repo (roots are empty)
    let root_evidence_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM repository_identity_evidence WHERE repository_id = ?1 AND kind = 'root_set'",
            [&repo_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(root_evidence_count, 0);
}
