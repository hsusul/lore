//! Landing evidence end to end, against real Git repositories.
//!
//! The claim under test is that content an agent created can be tied to a
//! commit — and, just as importantly, that Lore says nothing when it cannot.
//! These use real `git` because the whole mechanism rests on Git's own content
//! addressing; a mock would be testing the mock.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

use lore_core::landing::{self, Landing};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("git must be installed");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    std::fs::write(dir.path().join("seed.txt"), "seed\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "seed"]);
    dir
}

#[test]
fn our_oid_is_the_one_git_computes() {
    // The foundation. If this drifts, every landing answer is meaningless.
    let dir = repo();
    let content = b"fn main() { println!(\"hi\"); }\n";
    let ours = landing::blob_oid(content);

    std::fs::write(dir.path().join("m.rs"), content).unwrap();
    let theirs = Command::new("git")
        .args(["hash-object", "m.rs"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(ours, String::from_utf8_lossy(&theirs.stdout).trim());
}

#[test]
fn committed_content_is_found_in_a_commit() {
    let dir = repo();
    let content = b"pub fn added() {}\n";
    std::fs::write(dir.path().join("added.rs"), content).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "add"]);

    assert_eq!(
        landing::lookup_in_worktree(dir.path(), &landing::blob_oid(content)),
        Some(Landing::Committed)
    );
}

#[test]
fn staged_but_uncommitted_content_is_not_reported_as_committed() {
    // The distinction that makes the ladder worth having: the object exists, so
    // "no observed landing" would be wrong, but no branch carries it, so
    // "committed" would be a lie.
    let dir = repo();
    let content = b"pub fn staged_only() {}\n";
    std::fs::write(dir.path().join("staged.rs"), content).unwrap();
    git(dir.path(), &["add", "staged.rs"]);

    assert_eq!(
        landing::lookup_in_worktree(dir.path(), &landing::blob_oid(content)),
        Some(Landing::Staged)
    );
}

#[test]
fn content_never_given_to_git_has_no_observed_landing() {
    let dir = repo();
    assert_eq!(
        landing::lookup_in_worktree(dir.path(), &landing::blob_oid(b"never written\n")),
        Some(Landing::NoObservedLanding)
    );
}

#[test]
fn a_landing_claim_survives_a_rebase() {
    // The property that justifies addressing content instead of commits: a
    // rebase rewrites every commit id, and the blob id is unchanged.
    let dir = repo();
    let content = b"pub fn rebased() {}\n";

    git(dir.path(), &["checkout", "-b", "feature"]);
    std::fs::write(dir.path().join("f.rs"), content).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "feature work"]);
    let before = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // Move main forward, then rebase the feature commit onto it.
    git(dir.path(), &["checkout", "main"]);
    std::fs::write(dir.path().join("other.txt"), "other\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "main moves"]);
    git(dir.path(), &["checkout", "feature"]);
    git(dir.path(), &["rebase", "main"]);

    let after = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_ne!(
        before.stdout, after.stdout,
        "the rebase did not rewrite HEAD"
    );

    assert_eq!(
        landing::lookup_in_worktree(dir.path(), &landing::blob_oid(content)),
        Some(Landing::Committed),
        "a rebase broke a landing claim it should have survived"
    );
}

#[test]
fn a_landing_claim_survives_a_squash_merge() {
    let dir = repo();
    let content = b"pub fn squashed() {}\n";
    git(dir.path(), &["checkout", "-b", "topic"]);
    std::fs::write(dir.path().join("s.rs"), content).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "topic work"]);
    git(dir.path(), &["checkout", "main"]);
    git(dir.path(), &["merge", "--squash", "topic"]);
    git(dir.path(), &["commit", "-m", "squashed topic"]);
    git(dir.path(), &["branch", "-D", "topic"]);

    assert_eq!(
        landing::lookup_in_worktree(dir.path(), &landing::blob_oid(content)),
        Some(Landing::Committed),
        "a squash merge broke a landing claim"
    );
}

#[test]
fn content_edited_before_committing_reports_no_observed_landing() {
    // The honest limit of exact-content matching, pinned so nobody mistakes it
    // for a bug later: the agent's exact bytes never reached Git, so Lore
    // reports an absence of observation — not that the work was lost.
    let dir = repo();
    let agent_wrote = b"pub fn draft() {}\n";
    std::fs::write(dir.path().join("d.rs"), agent_wrote).unwrap();
    // A human tweaks it before committing.
    std::fs::write(dir.path().join("d.rs"), b"pub fn draft() { /* tweak */ }\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "tweaked"]);

    assert_eq!(
        landing::lookup_in_worktree(dir.path(), &landing::blob_oid(agent_wrote)),
        Some(Landing::NoObservedLanding)
    );
}

#[test]
fn a_deleted_worktree_does_not_erase_a_landing_claim() {
    // Agents work in throwaway worktrees. Linked worktrees share one object
    // database, so the commits survive in the primary — looking only at the
    // path the agent used would report every finished task as unlanded.
    let dir = repo();
    let holder = tempfile::tempdir().unwrap();
    let linked = holder.path().join("wt");
    git(
        dir.path(),
        &["worktree", "add", "-b", "side", linked.to_str().unwrap()],
    );

    let content = b"pub fn from_a_worktree() {}\n";
    std::fs::write(linked.join("w.rs"), content).unwrap();
    git(&linked, &["add", "."]);
    git(&linked, &["commit", "-m", "work in a worktree"]);

    // The worktree goes away, as they do.
    std::fs::remove_dir_all(&linked).unwrap();
    git(dir.path(), &["worktree", "prune"]);

    assert_eq!(
        landing::lookup_in_worktree(dir.path(), &landing::blob_oid(content)),
        Some(Landing::Committed),
        "work committed from a since-deleted worktree was reported as unlanded"
    );
}

#[test]
fn ingest_records_a_content_oid_for_creates_and_not_for_edits() {
    // The migration's contract: an oid exists exactly where the resulting
    // content is genuinely known.
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = lore_core::storage::blob::BlobStore::open(blob_dir.path()).unwrap();

    let fixture = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/codex/patch_apply.jsonl"),
    )
    .unwrap();
    let parsed = lore_core::adapters::codex::CodexAdapter::new().parse_str(&fixture, "landing");
    let sid = lore_core::ingest::persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();

    let mut stmt = conn
        .prepare(
            "SELECT change_kind, content_oid, content_oid_algo FROM file_event
             WHERE session_id = ?1",
        )
        .unwrap();
    let rows: Vec<(String, Option<String>, Option<String>)> = stmt
        .query_map([&sid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!rows.is_empty(), "the fixture records file changes");

    for (kind, oid, algo) in &rows {
        match kind.as_str() {
            "create" => {
                let oid = oid.as_ref().expect("a create knows its resulting content");
                assert_eq!(oid.len(), 40, "not a git sha1 oid: {oid}");
                assert_eq!(algo.as_deref(), Some("git-sha1"));
            }
            // Everything else records a diff or a fragment, so there is no
            // honest oid to compute.
            _ => assert!(
                oid.is_none(),
                "`{kind}` recorded an oid it cannot know: {oid:?}"
            ),
        }
    }
}

#[test]
fn an_ingested_create_can_be_found_in_the_repository_it_landed_in() {
    // The full loop: ingest records an oid, the same bytes are committed, and
    // the resolver finds them.
    let dir = repo();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = lore_core::storage::blob::BlobStore::open(blob_dir.path()).unwrap();

    let fixture = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/codex/patch_apply.jsonl"),
    )
    .unwrap();
    let parsed = lore_core::adapters::codex::CodexAdapter::new().parse_str(&fixture, "landing2");
    let sid = lore_core::ingest::persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();

    let (oid, relpath): (String, String) = conn
        .query_row(
            "SELECT content_oid, path FROM file_event
             WHERE session_id = ?1 AND content_oid IS NOT NULL LIMIT 1",
            [&sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    // Before the content is committed, it is simply not observed.
    assert_eq!(
        landing::lookup_in_worktree(dir.path(), &oid),
        Some(Landing::NoObservedLanding)
    );

    // Commit exactly the bytes the agent recorded.
    let content: Vec<u8> = conn
        .query_row(
            "SELECT b.storage_relpath FROM file_event f JOIN blob b ON b.id=f.patch_blob_id
             WHERE f.session_id=?1 AND f.content_oid=?2",
            rusqlite::params![&sid, &oid],
            |r| r.get::<_, String>(0),
        )
        .map(|rel| blobs.read(&rel).unwrap())
        .unwrap();
    let name = Path::new(&relpath).file_name().unwrap();
    std::fs::write(dir.path().join(name), &content).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "land the agent's file"]);

    assert_eq!(
        landing::lookup_in_worktree(dir.path(), &oid),
        Some(Landing::Committed),
        "the ingested oid did not match the committed content"
    );
}

#[test]
fn content_that_landed_and_was_later_changed_still_counts_as_landed() {
    // Regression for a flaw in the first implementation, which searched only the
    // tips' trees. Work that was committed and then edited would have reported
    // "no observed landing" — the most damaging possible error, since it denies
    // work that demonstrably shipped.
    let dir = repo();
    let original = b"pub fn first_version() {}\n";
    std::fs::write(dir.path().join("evolving.rs"), original).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "the agent's version"]);

    // Someone edits it later and commits again; the original is now history.
    std::fs::write(
        dir.path().join("evolving.rs"),
        b"pub fn second_version() {}\n",
    )
    .unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "later change"]);

    assert_eq!(
        landing::lookup_in_worktree(dir.path(), &landing::blob_oid(original)),
        Some(Landing::Committed),
        "content that was committed and later superseded was reported as unlanded"
    );
}

#[test]
fn the_index_is_built_once_and_answers_many() {
    // `status` classifies every recorded change in a repository, so the walk
    // must be amortized rather than repeated per oid.
    let dir = repo();
    let a = b"pub fn a() {}\n";
    let b = b"pub fn b() {}\n";
    std::fs::write(dir.path().join("a.rs"), a).unwrap();
    std::fs::write(dir.path().join("b.rs"), b).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "two files"]);

    let index = landing::LandingIndex::build(dir.path()).expect("index builds");
    assert!(!index.is_empty(), "the index found no blobs at all");
    assert!(index.contains(&landing::blob_oid(a)));
    assert!(index.contains(&landing::blob_oid(b)));
    assert!(!index.contains(&landing::blob_oid(b"never committed\n")));
}
