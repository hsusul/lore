//! Claude Code puts harness text in user turns: slash command echoes,
//! system reminders, local command output. They are not the person's words.
//! A turn made only of such blocks is skipped, and the blocks are stripped
//! from a turn that also has real text.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, find_conflicts, lessons, read_turns};

fn sid(n: u32) -> String {
    format!("cccccccc-0000-4000-8000-{n:012x}")
}

#[test]
fn harness_blocks_are_not_the_persons_words() {
    let dir = tempfile::tempdir().unwrap();
    let ids: Vec<String> = (1..=6).map(sid).collect();
    let conn = archive::claude_archive(
        dir.path(),
        &[
            (
                &ids[0],
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
            (
                &ids[1],
                &[(
                    "user",
                    "<command-message>review is running</command-message>\n\
                     <command-name>/review</command-name>\n\
                     <command-args>open a PR for it</command-args>",
                    "2026-08-02T10:00:00.000Z",
                )],
            ),
            (
                &ids[2],
                &[(
                    "user",
                    "<system-reminder>The PR checks are failing.</system-reminder>",
                    "2026-08-03T10:00:00.000Z",
                )],
            ),
            (
                &ids[3],
                &[(
                    "user",
                    "<local-command-stdout>pushed the pr branch</local-command-stdout>",
                    "2026-08-04T10:00:00.000Z",
                )],
            ),
            (
                &ids[4],
                &[
                    ("user", "go", "2026-08-05T10:00:00.000Z"),
                    (
                        "assistant",
                        "Running npm install.",
                        "2026-08-05T10:00:01.000Z",
                    ),
                    (
                        "user",
                        "<system-reminder>a PR is open</system-reminder> no, use pnpm not npm",
                        "2026-08-05T10:00:02.000Z",
                    ),
                ],
            ),
            (
                &ids[5],
                &[(
                    "user",
                    "please open a PR for the exif branch",
                    "2026-08-06T10:00:00.000Z",
                )],
            ),
        ],
    );
    let found = corrections(&conn).unwrap();
    let mut texts: Vec<&str> = found.iter().map(|c| c.text.as_str()).collect();
    texts.sort_unstable();
    assert_eq!(
        texts,
        ["dont make a pr just push to main", "no, use pnpm not npm"]
    );

    let mut found = lessons(found);
    find_conflicts(&read_turns(&conn).unwrap(), &mut found);
    let pr = found
        .iter()
        .find(|l| l.text == "dont make a pr just push to main")
        .unwrap();
    let listed: Vec<(String, &str)> = pr
        .conflicts
        .iter()
        .map(|c| (c.session.clone(), c.text.as_str()))
        .collect();
    assert_eq!(
        listed,
        [(
            archive::claude_session_id(&conn, &ids[5]),
            "please open a PR for the exif branch"
        )]
    );
}
