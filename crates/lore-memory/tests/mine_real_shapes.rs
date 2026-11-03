//! Turns from a real history. The miner ranked the first three as lessons
//! though none is one; the person accepted the other four.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, lessons};

/// Said in real sessions and not lessons: a question with no question mark,
/// an explanation, and a one-off task with a rule tacked on.
const NOT_LESSONS: &[&str] = &[
    "wait so then what would be our wanted result",
    "a worktree is jsut a seperate branch of a codebase that you can work on without touching the main one",
    "delete the ios sim runtimes and make it so that it builds them on my ssd from now on",
];

/// Said in real sessions and accepted as lessons.
const LESSONS: &[&str] = &[
    "make sure that docs are never tracked",
    "Stop asking me for permission",
    "never use urself as a contributor",
    "dont make a doc for it just give me the thing to copy from now on",
];

#[test]
fn real_non_lessons_are_left_out_and_real_lessons_kept() {
    let dir = tempfile::tempdir().unwrap();
    let sessions: Vec<Vec<(&str, &str)>> = NOT_LESSONS
        .iter()
        .chain(LESSONS)
        .map(|text| {
            vec![
                ("user", "go"),
                ("assistant", "Done, I pushed the change."),
                ("user", *text),
            ]
        })
        .collect();
    let refs: Vec<&[(&str, &str)]> = sessions.iter().map(Vec::as_slice).collect();
    let conn = archive::codex_archive(dir.path(), &refs);
    let mut found: Vec<String> = lessons(corrections(&conn).unwrap())
        .into_iter()
        .filter(|l| l.score >= 0)
        .map(|l| l.text)
        .collect();
    found.sort();
    let mut expected: Vec<String> = LESSONS.iter().map(|s| (*s).to_owned()).collect();
    expected.sort();
    assert_eq!(found, expected);
}
