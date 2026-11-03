//! Later user turns about the same command family are listed beside a lesson
//! as turns that may conflict with it. Lore does not judge them; the agent
//! and the person do.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, find_conflicts, lessons};

#[test]
fn later_turns_about_the_same_command_are_listed_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let conn = archive::codex_archive(
        dir.path(),
        &[
            // Before the lesson: not listed.
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
            // A different meaning of a doc: not the docs family.
            &[("user", "make a doc of it")],
            &[(
                "user",
                "You are allowed to merge/create prs dont ask me for permission",
            )],
            &[("user", "please open a PR for the exif branch")],
            &[("user", "now push to main and open a pull request too")],
            &[("user", "a PR for the readme, please")],
        ],
    );
    let mut found = lessons(corrections(&conn).unwrap());
    find_conflicts(&conn, &mut found).unwrap();
    let listed: Vec<(String, Vec<(String, String)>)> = found
        .iter()
        .map(|l| {
            let turns = l
                .conflicts
                .iter()
                .map(|c| (c.session.clone(), c.text.clone()))
                .collect();
            (l.text.clone(), turns)
        })
        .collect();
    let s = |n| archive::session_id(&conn, n);
    assert_eq!(
        listed,
        [
            ("make sure that docs are never tracked".to_owned(), vec![]),
            (
                "dont make a pr just push to main".to_owned(),
                vec![
                    (s(8), "a PR for the readme, please".to_owned()),
                    (
                        s(7),
                        "now push to main and open a pull request too".to_owned()
                    ),
                    (s(6), "please open a PR for the exif branch".to_owned()),
                ]
            ),
        ]
    );
}
