//! Anchors pinned to a scratch repository's working tree: the hash and commit
//! filled in, and missing files and lines refused.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{commit, repo};
use lore_memory::anchor::{hash_lines, PinError, WorkTree};
use lore_memory::note::{Anchor, LineHash, LineRange, RelPath};

fn lines(s: &str) -> LineRange {
    LineRange::parse(s).unwrap()
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
