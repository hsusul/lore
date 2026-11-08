//! Claude Code puts harness text in user turns: slash command echoes,
//! system reminders, local command output. They are not the person's words.
//! A turn made only of such blocks is skipped, and the blocks are stripped
//! from a turn that also has real text. The words typed after a slash
//! command, in `<command-args>`, are the person's and are kept.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{
    corrections, find_conflicts, is_harness_text, lessons, persons_words, read_turns,
};

#[test]
fn is_harness_text_names_what_the_harness_wrote_in_a_user_turn() {
    for text in [
        "This session is being continued from a previous conversation.",
        "Base directory for this skill: /skills/example",
        "[Request interrupted by user]",
        "Another Claude session sent a message: hello",
        "(Re-invocation of /review)",
        "[Image #1] [Image #2]",
        "  <pasted-content>notes</pasted-content>",
        "I hit my usage limit while you were working, carry on from there.",
        "[Usage limit reached; wrap up the step you are on.]",
        "[Earlier usage-limit notes are void. Keep going.]",
    ] {
        assert!(is_harness_text(text), "{text}");
    }
    for text in ["pin the schema version first", "[Image #1] like this", ""] {
        assert!(!is_harness_text(text), "{text}");
    }
}

#[test]
fn persons_words_drops_what_the_host_wrote() {
    for raw in [
        "<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n\
         <summary>Agent finished</summary>\n</task-notification>",
        "<system-reminder>a PR is open</system-reminder>\n  ",
        "<local-command-stdout>ok</local-command-stdout>",
        "I hit my usage limit while you were working, carry on from there.",
        "",
    ] {
        assert_eq!(persons_words(raw), None, "{raw}");
    }
    assert_eq!(
        persons_words(
            "<task-notification><task-id>a1</task-id></task-notification>\n\
             retry the migration"
        )
        .as_deref(),
        Some("retry the migration")
    );
    assert_eq!(
        persons_words("no, use pnpm not npm").as_deref(),
        Some("no, use pnpm not npm")
    );
}

#[test]
fn a_usage_limit_note_is_not_mined() {
    let dir = tempfile::tempdir().unwrap();
    let ids: Vec<String> = (11..=13).map(sid).collect();
    let note = "I hit my usage limit while you were working, but it is back now. \
                Please continue where you stopped.";
    let grace = "[Usage limit reached; a little time remains, so stop after this step.]";
    let lifted = "[Earlier usage-limit notes no longer hold. Don't stop, carry on.]";
    let conn = archive::claude_archive(
        dir.path(),
        &[
            (
                &ids[0],
                &[
                    ("user", "go", "2026-08-01T10:00:00.000Z"),
                    ("assistant", "Working on it.", "2026-08-01T10:00:01.000Z"),
                    ("user", note, "2026-08-01T10:00:02.000Z"),
                    ("assistant", "Resuming.", "2026-08-01T10:00:03.000Z"),
                    ("user", grace, "2026-08-01T10:00:04.000Z"),
                    ("assistant", "Stopping here.", "2026-08-01T10:00:05.000Z"),
                    ("user", lifted, "2026-08-01T10:00:06.000Z"),
                ],
            ),
            (
                &ids[1],
                &[
                    ("user", "go", "2026-08-02T10:00:00.000Z"),
                    ("assistant", "Working on it.", "2026-08-02T10:00:01.000Z"),
                    ("user", note, "2026-08-02T10:00:02.000Z"),
                ],
            ),
            (
                &ids[2],
                &[
                    ("user", "go", "2026-08-03T10:00:00.000Z"),
                    (
                        "assistant",
                        "Running npm install.",
                        "2026-08-03T10:00:01.000Z",
                    ),
                    ("user", "no, use pnpm not npm", "2026-08-03T10:00:02.000Z"),
                ],
            ),
        ],
    );
    let found = corrections(&conn).unwrap();
    let texts: Vec<&str> = found.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["no, use pnpm not npm"]);
}

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
        [
            (
                archive::claude_session_id(&conn, &ids[5]),
                "please open a PR for the exif branch"
            ),
            (
                archive::claude_session_id(&conn, &ids[1]),
                "open a PR for it"
            ),
        ]
    );
}
