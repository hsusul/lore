//! The cost of the in-file move search for a file just under the 1 MiB cap.
//!
//! `anchor::find_lines` hashes every window of `len` lines in the file. The
//! 1 MiB cap bounds the bytes it reads but not the work it does, because the
//! work is `(lines - len) * len` line hashes. This file is 600,000 bytes, well
//! under the cap, and the anchored block is 30,000 lines, so a check that has
//! to search does about 9e8 line hashes.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::fs;
use std::time::{Duration, Instant};

use common::{commit, repo};
use lore_memory::anchor::{Freshness, WorkTree, MOVE_SEARCH_LIMIT};
use lore_memory::note::{Anchor, LineRange, RelPath};

/// 10,000 filler lines then 30,000 anchored lines, about 600 KB.
fn body() -> String {
    let mut text = String::new();
    for i in 0..10_000 {
        text.push_str(&format!("filler-{i}\n"));
    }
    for i in 0..30_000 {
        text.push_str(&format!("anchored-{i}\n"));
    }
    text
}

/// A repo with `big.txt` committed and an anchor pinned to its 30,000
/// anchored lines.
fn pinned(root: &std::path::Path) -> (String, Anchor) {
    repo(root, None);
    let text = body();
    assert!(
        text.len() as u64 <= MOVE_SEARCH_LIMIT,
        "the fixture must stay under the cap, it is {} bytes",
        text.len()
    );
    commit(root, "big.txt", &text, "big");

    let tree = WorkTree::new(root);
    let anchor = tree
        .pin(&Anchor {
            path: RelPath::parse("big.txt").unwrap(),
            lines: Some(LineRange::parse("10001-40000").unwrap()),
            hash: None,
            commit: None,
        })
        .unwrap();
    assert_eq!(anchor.check(root), Freshness::Fresh);
    (text, anchor)
}

/// Check `anchor`, print how long it took, and fail past two seconds.
fn timed_check(root: &std::path::Path, anchor: &Anchor) -> Freshness {
    let started = Instant::now();
    let freshness = anchor.check(root);
    let elapsed = started.elapsed();
    eprintln!(
        "move search over 30000 anchored lines in a {} byte file took {elapsed:?}, {freshness:?}, load {}",
        fs::metadata(root.join("big.txt")).unwrap().len(),
        std::fs::read_to_string("/proc/loadavg").unwrap_or_else(|_| "n/a".into()).trim()
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "a move search inside the 1 MiB cap took {elapsed:?}"
    );
    freshness
}

#[test]
fn a_move_search_in_a_file_near_the_cap_finishes_in_two_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (text, anchor) = pinned(root);

    // One line inserted at the top. The anchored block is intact at 10002-40001.
    fs::write(root.join("big.txt"), format!("inserted\n{text}")).unwrap();
    assert_eq!(
        timed_check(root, &anchor),
        Freshness::Moved {
            lines: LineRange::parse("10002-40001").unwrap()
        }
    );
}

#[test]
fn a_failed_search_in_a_file_near_the_cap_finishes_in_two_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (text, anchor) = pinned(root);

    // The last anchored line edited, so no run of the anchored lines is left.
    let edited = text.replace("anchored-29999\n", "edited\n");
    fs::write(root.join("big.txt"), edited).unwrap();
    assert_eq!(timed_check(root, &anchor), Freshness::Changed);
}
