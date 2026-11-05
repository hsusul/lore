//! A resume or fork copies a conversation into a new session, timestamps
//! included. A later turn copied that way is one possible conflict, not two.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, find_conflicts, lessons, read_turns};

#[test]
fn a_conflict_copied_into_a_fork_is_listed_once() {
    let dir = tempfile::tempdir().unwrap();
    let (lesson, original, fork) = (
        "cccccccc-0000-4000-8000-000000000001",
        "cccccccc-0000-4000-8000-000000000002",
        "cccccccc-0000-4000-8000-000000000003",
    );
    let later: &[archive::Turn<'_>] = &[(
        "user",
        "please open a PR for the exif branch",
        "2026-08-02T10:00:00.000Z",
    )];
    let conn = archive::claude_archive(
        dir.path(),
        &[
            (
                lesson,
                &[
                    ("user", "go", "2026-08-01T10:00:00.000Z"),
                    ("assistant", "Opened a PR.", "2026-08-01T10:00:01.000Z"),
                    (
                        "user",
                        "dont make a pr just push to main",
                        "2026-08-01T10:00:02.000Z",
                    ),
                ],
            ),
            (original, later),
            (fork, later),
        ],
    );
    let mut found = lessons(corrections(&conn).unwrap());
    find_conflicts(&read_turns(&conn).unwrap(), &mut found);
    let listed: Vec<(String, &str)> = found[0]
        .conflicts
        .iter()
        .map(|c| (c.session.clone(), c.text.as_str()))
        .collect();
    let first = [original, fork]
        .map(|uuid| archive::claude_session_id(&conn, uuid))
        .into_iter()
        .min()
        .unwrap();
    assert_eq!(listed, [(first, "please open a PR for the exif branch")]);
}
