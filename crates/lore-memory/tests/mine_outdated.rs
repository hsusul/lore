//! A lesson a later session contradicts is marked possibly outdated, with
//! that session as a receipt: same command family, opposite instruction,
//! newer date.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, lessons, mark_outdated};

#[test]
fn a_later_opposite_instruction_marks_a_lesson_possibly_outdated() {
    let dir = tempfile::tempdir().unwrap();
    let conn = archive::codex_archive(
        dir.path(),
        &[
            // Before the lesson: an earlier PR request does not outdate it.
            &[("user", "open a PR for the parser fix")],
            &[
                ("user", "go"),
                ("assistant", "Opened a PR."),
                ("user", "dont make a pr just push to main"),
            ],
            &[
                ("user", "go"),
                ("assistant", "Committed docs/."),
                ("user", "make sure that docs are never tracked"),
            ],
            // Same family, same instruction: not a contradiction.
            &[("user", "dont make a pr for this one either")],
            &[("user", "dont track the docs folder")],
            // Same family, opposite instruction, newer: a contradiction.
            &[("user", "please open a PR for the exif branch")],
            &[("user", "now push to main and open a pull request too")],
        ],
    );
    let mut found = lessons(corrections(&conn).unwrap());
    mark_outdated(&conn, &mut found).unwrap();
    let marks: Vec<(String, Option<(String, String)>)> = found
        .iter()
        .map(|l| {
            let by = (l.outdated_by.as_ref()).map(|r| (r.session.clone(), r.text.clone()));
            (l.text.clone(), by)
        })
        .collect();
    let s = |n| archive::session_id(&conn, n);
    assert_eq!(
        marks,
        [
            ("make sure that docs are never tracked".to_owned(), None),
            (
                "dont make a pr just push to main".to_owned(),
                Some((
                    s(7),
                    "now push to main and open a pull request too".to_owned()
                )),
            ),
        ]
    );
}
