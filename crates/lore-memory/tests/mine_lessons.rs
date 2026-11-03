//! Lessons: candidates from different sessions that say the same thing,
//! ranked by how many sessions said it, each keeping its receipts.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, lessons};

#[test]
fn corrections_from_two_or_more_sessions_become_a_ranked_lesson() {
    let dir = tempfile::tempdir().unwrap();
    let conn = archive::codex_archive(
        dir.path(),
        &[
            &[
                ("user", "add axios"),
                ("assistant", "Running npm install axios."),
                ("user", "no, use pnpm not npm"),
            ],
            &[
                ("user", "add a test"),
                ("assistant", "I'll edit src/gen.rs."),
                ("user", "don't edit the generated files"),
            ],
            &[
                ("user", "add lodash"),
                ("assistant", "Running npm install lodash."),
                ("user", "wrong tool, use pnpm instead of npm"),
            ],
            &[
                ("user", "fix the build"),
                ("assistant", "Edited src/gen.rs."),
                ("user", "stop editing the generated files"),
            ],
            &[
                ("user", "add dayjs"),
                ("assistant", "npm install dayjs done."),
                ("user", "I told you, pnpm not npm"),
            ],
            &[
                ("user", "rename the module"),
                ("assistant", "Renamed it."),
                ("user", "no, keep the old name for the export"),
            ],
        ],
    );
    let found = lessons(corrections(&conn).unwrap());
    let summary: Vec<(usize, &str, Vec<String>)> = found
        .iter()
        .map(|l| {
            let sessions = l.receipts.iter().map(|r| r.session.clone()).collect();
            (l.sessions, l.text.as_str(), sessions)
        })
        .collect();
    let s = |n| archive::session_id(&conn, n);
    assert_eq!(
        summary,
        [
            (3, "I told you, pnpm not npm", vec![s(5), s(3), s(1)]),
            (2, "stop editing the generated files", vec![s(4), s(2)]),
        ]
    );
    // The id is stable for the same candidates, and distinct per lesson.
    let again = lessons(corrections(&conn).unwrap());
    assert_eq!(found[0].id, again[0].id);
    assert_ne!(found[0].id, found[1].id);
    assert_eq!(found[0].id.len(), 10);
    assert_eq!(found[0].latest_ms, Some(1_785_924_003_000));
}

#[test]
fn a_correction_said_in_one_session_only_is_not_a_lesson() {
    let dir = tempfile::tempdir().unwrap();
    let conn = archive::codex_archive(
        dir.path(),
        &[&[
            ("user", "add axios"),
            ("assistant", "Running npm install axios."),
            ("user", "no, use pnpm not npm"),
            ("assistant", "Running npm install again."),
            ("user", "I said pnpm not npm"),
        ]],
    );
    assert_eq!(corrections(&conn).unwrap().len(), 2);
    assert!(lessons(corrections(&conn).unwrap()).is_empty());
}
