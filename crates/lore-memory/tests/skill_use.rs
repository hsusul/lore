//! Skill usage: how often each skill ran, and the corrections the person made
//! right after it, from the archive.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use archive::claude_archive;
use lore_memory::mine::{self, Limits};
use lore_memory::skill_use::skill_usage;

const A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
const C: &str = "cccccccc-0000-4000-8000-000000000003";
const D: &str = "dddddddd-0000-4000-8000-000000000004";

fn usage(conn: &rusqlite::Connection) -> Vec<(String, usize, usize, Vec<String>)> {
    let turns = mine::read_turns(conn).unwrap();
    skill_usage(conn, &turns, &Limits::default())
        .unwrap()
        .into_iter()
        .map(|u| {
            (
                u.skill,
                u.uses,
                u.sessions,
                u.reprompts.into_iter().map(|c| c.text).collect(),
            )
        })
        .collect()
}

#[test]
fn a_correction_right_after_a_skill_counts_against_that_skill() {
    let dir = tempfile::tempdir().unwrap();
    let conn = claude_archive(
        dir.path(),
        &[
            (A, &[
                ("user", "review the diff", "2026-09-01T10:00:00Z"),
                ("skill", "code-review", "2026-09-01T10:00:05Z"),
                ("assistant", "Reviewed it, looks fine to me.", "2026-09-01T10:01:00Z"),
                ("user", "no, you missed the tests, look again", "2026-09-01T10:02:00Z"),
            ]),
            (B, &[
                ("user", "<command-name>/poteto-mode</command-name>\n<command-args>build the thing</command-args>", "2026-09-02T10:00:00Z"),
                ("assistant", "Built it.", "2026-09-02T10:01:00Z"),
                ("user", "thanks, looks good", "2026-09-02T10:02:00Z"),
            ]),
            (C, &[
                ("user", "review this one too", "2026-09-03T10:00:00Z"),
                ("skill", "code-review", "2026-09-03T10:00:05Z"),
                ("assistant", "Reviewed, two issues.", "2026-09-03T10:01:00Z"),
                ("user", "great, fix them", "2026-09-03T10:02:00Z"),
                ("assistant", "Fixed both.", "2026-09-03T10:03:00Z"),
                ("user", "(Re-invocation of /code-review \u{2014} the skill instructions were previously loaded; the arguments or dynamic output below are new.)", "2026-09-03T10:04:00Z"),
                ("assistant", "Reviewed again, clean.", "2026-09-03T10:05:00Z"),
            ]),
        ],
    );
    assert_eq!(
        usage(&conn),
        [
            (
                "code-review".to_owned(),
                3,
                2,
                vec!["no, you missed the tests, look again".to_owned()]
            ),
            ("poteto-mode".to_owned(), 1, 1, vec![]),
        ]
    );
}

#[test]
fn a_correction_long_after_the_skill_is_not_held_against_it() {
    let dir = tempfile::tempdir().unwrap();
    let conn = claude_archive(
        dir.path(),
        &[(
            D,
            &[
                ("user", "check the build", "2026-09-04T10:00:00Z"),
                ("skill", "verify-build", "2026-09-04T10:00:05Z"),
                ("assistant", "The build passes.", "2026-09-04T10:01:00Z"),
                (
                    "user",
                    "good, now rename the module",
                    "2026-09-04T10:02:00Z",
                ),
                ("assistant", "Renamed it.", "2026-09-04T10:03:00Z"),
                ("user", "update the docs as well", "2026-09-04T10:04:00Z"),
                ("assistant", "Docs updated.", "2026-09-04T10:05:00Z"),
                ("user", "and add a changelog entry", "2026-09-04T10:06:00Z"),
                ("assistant", "Added it with npm.", "2026-09-04T10:07:00Z"),
                ("user", "no, use pnpm not npm", "2026-09-04T10:08:00Z"),
            ],
        )],
    );
    assert_eq!(usage(&conn), [("verify-build".to_owned(), 1, 1, vec![])]);
}
