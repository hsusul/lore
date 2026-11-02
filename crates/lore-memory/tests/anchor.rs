//! One anchor checked against each commit of a three-commit fixture repo, and
//! against the working tree after a delete and after a `git mv`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::fs;
use std::path::Path;

use common::{commit, git, repo};
use lore_memory::anchor::{hash_lines, Freshness, PinError, WorkTree, MOVE_SEARCH_LIMIT};
use lore_memory::note::{Anchor, LineHash, LineRange, RelPath};

const FIRST: &str = "\
// migrations
fn a() {}
fn anchored() {
    retry_migration();
}
fn z() {}
";

/// Two lines inserted above the anchored function.
const SECOND: &str = "\
// one
// two
// migrations
fn a() {}
fn anchored() {
    retry_migration();
}
fn z() {}
";

/// The anchored function's body edited.
const THIRD: &str = "\
// one
// two
// migrations
fn a() {}
fn anchored() {
    skip_migration();
}
fn z() {}
";

fn lines(s: &str) -> LineRange {
    LineRange::parse(s).unwrap()
}

/// The fixture: three commits of `src/lib.rs`, and the anchor pinned at the
/// first one.
fn fixture(dir: &Path) -> ([String; 3], Anchor) {
    repo(dir, Some("git@github.com:example/fixture.git"));
    let first = commit(dir, "src/lib.rs", FIRST, "first");
    let anchor = WorkTree::new(dir)
        .pin(&Anchor {
            path: RelPath::parse("src/lib.rs").unwrap(),
            lines: Some(lines("3-5")),
            hash: None,
            commit: None,
        })
        .unwrap();
    let second = commit(dir, "src/lib.rs", SECOND, "second");
    let third = commit(dir, "src/lib.rs", THIRD, "third");
    ([first, second, third], anchor)
}

#[test]
fn anchor_freshness_at_each_fixture_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let ([first, second, third], anchor) = fixture(root);

    assert_eq!(
        anchor.hash,
        Some(LineHash::parse("b3:486dbfb6d60f46a7").unwrap())
    );
    assert_eq!(anchor.commit.as_ref().unwrap().as_str(), &first[..12]);

    let mut seen = Vec::new();
    for commit in [&first, &second, &third] {
        git(root, &["checkout", "-q", commit]);
        seen.push(anchor.check(root));
    }
    assert_eq!(
        seen,
        [
            Freshness::Fresh,
            Freshness::Moved {
                lines: lines("5-7")
            },
            Freshness::Changed,
        ]
    );

    git(root, &["checkout", "-q", &third]);
    fs::remove_file(root.join("src/lib.rs")).unwrap();
    assert_eq!(anchor.check(root), Freshness::Gone);
}

#[test]
fn a_git_mv_reports_moved_at_the_new_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let ([first, ..], anchor) = fixture(root);
    git(root, &["checkout", "-q", &first]);
    git(root, &["mv", "--", "src/lib.rs", "src/core.rs"]);

    let located = WorkTree::new(root).locate(&anchor);
    assert_eq!(
        located.freshness,
        Freshness::Moved {
            lines: lines("3-5")
        }
    );
    assert_eq!(located.path.as_str(), "src/core.rs");
    assert!(
        git(root, &["status", "--porcelain"]).starts_with("R  src/lib.rs -> src/core.rs"),
        "the check left the rename staged and nothing else"
    );
}

#[test]
fn anchors_without_a_hash_check_what_they_can() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repo(root, None);
    commit(root, "a.txt", "one\ntwo\n", "a");
    let anchor = |path: &str, range: Option<&str>| Anchor {
        path: RelPath::parse(path).unwrap(),
        lines: range.map(lines),
        hash: None,
        commit: None,
    };
    assert_eq!(anchor("a.txt", None).check(root), Freshness::Fresh);
    assert_eq!(anchor("a.txt", Some("2")).check(root), Freshness::Fresh);
    assert_eq!(anchor("a.txt", Some("2-3")).check(root), Freshness::Changed);
    assert_eq!(anchor("b.txt", None).check(root), Freshness::Gone);
}

#[test]
fn pinning_fills_the_hash_and_the_head_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repo(root, None);
    let head = commit(root, "a.txt", "one\ntwo\r\n", "a");
    let tree = WorkTree::new(root);
    let anchor = |range: Option<&str>| Anchor {
        path: RelPath::parse("a.txt").unwrap(),
        lines: range.map(lines),
        hash: None,
        commit: None,
    };
    let pinned = tree.pin(&anchor(Some("2"))).unwrap();
    let digest = hash_lines([&b"two"[..]]);
    assert_eq!(
        pinned.hash,
        Some(LineHash::parse(&format!("b3:{}", &digest[..16])).unwrap())
    );
    assert_eq!(pinned.commit.unwrap().as_str(), &head[..12]);
    let path_only = tree.pin(&anchor(None)).unwrap();
    assert_eq!(path_only.hash, None);
    assert_eq!(path_only.commit.unwrap().as_str(), &head[..12]);
}

#[test]
fn pinning_refuses_missing_files_and_lines() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repo(root, None);
    commit(root, "a.txt", "one\n", "a");
    let tree = WorkTree::new(root);
    let anchor = |path: &str, range: &str| Anchor {
        path: RelPath::parse(path).unwrap(),
        lines: Some(lines(range)),
        hash: None,
        commit: None,
    };
    assert_eq!(
        tree.pin(&anchor("gone.txt", "1")),
        Err(PinError::Missing("gone.txt".into()))
    );
    assert_eq!(
        tree.pin(&anchor("a.txt", "2-4")),
        Err(PinError::OutOfRange {
            path: "a.txt".into(),
            lines: lines("2-4"),
            len: 1
        })
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_repo_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    repo(&root, None);
    fs::write(dir.path().join("outside.txt"), "secret\n").unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside.txt"), root.join("link.txt")).unwrap();
    let anchor = Anchor {
        path: RelPath::parse("link.txt").unwrap(),
        lines: None,
        hash: None,
        commit: None,
    };
    assert_eq!(anchor.check(&root), Freshness::Gone);
}

#[test]
fn lines_that_moved_in_a_file_over_1_mib_are_changed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repo(root, None);
    let padding = format!("{}\n", "x".repeat(99)).repeat(11_000);
    commit(root, "big.txt", &format!("anchored\n{padding}"), "big");
    commit(root, "small.txt", "anchored\nrest\n", "small");
    let tree = WorkTree::new(root);
    let pin = |path: &str| {
        tree.pin(&Anchor {
            path: RelPath::parse(path).unwrap(),
            lines: Some(lines("1")),
            hash: None,
            commit: None,
        })
        .unwrap()
    };
    let (big, small) = (pin("big.txt"), pin("small.txt"));
    fs::write(root.join("big.txt"), format!("new\nanchored\n{padding}")).unwrap();
    fs::write(root.join("small.txt"), "new\nanchored\nrest\n").unwrap();
    assert!(fs::metadata(root.join("big.txt")).unwrap().len() > MOVE_SEARCH_LIMIT);
    assert_eq!(small.check(root), Freshness::Moved { lines: lines("2") });
    assert_eq!(big.check(root), Freshness::Changed);
}
