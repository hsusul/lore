//! Words typed after a Claude Code slash command or skill, `/name some words`,
//! are the person's: Claude Code stores them in `<command-args>`, between a
//! `<command-message>` echo and whatever the harness loads next. The miner
//! reads only those words. A command with nothing typed after it is not a turn.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, read_turns, replies_in, Limits, Signal};

const S: &str = "eeeeeeee-0000-4000-8000-000000000001";

fn wrapped(name: &str, args: &str) -> String {
    format!(
        "<command-message>{name} is running\u{2026}</command-message>\n\
         <command-name>/{name}</command-name>\n\
         <command-args>{args}</command-args>"
    )
}

#[test]
fn typed_command_args_are_mined_and_bare_commands_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let correction = wrapped("tidy", "don't move the vendor folder, put it back");
    let approval = format!(
        "{}\nBase directory for this skill: /skills/tidy\n\nNever rename anything.",
        wrapped("tidy", "looks good, ship it")
    );
    let bare = wrapped("clear", "");
    let conn = archive::claude_archive(
        dir.path(),
        &[(
            S,
            &[
                ("user", "start", "2026-08-01T10:00:00.000Z"),
                (
                    "assistant",
                    "Moved the shared helpers into the vendor folder.",
                    "2026-08-01T10:00:01.000Z",
                ),
                ("user", &correction, "2026-08-01T10:00:02.000Z"),
                ("assistant", "Put it back.", "2026-08-01T10:00:03.000Z"),
                ("user", &approval, "2026-08-01T10:00:04.000Z"),
                ("assistant", "Shipped.", "2026-08-01T10:00:05.000Z"),
                ("user", &bare, "2026-08-01T10:00:06.000Z"),
                ("assistant", "Cleared.", "2026-08-01T10:00:07.000Z"),
                (
                    "user",
                    "Base directory for this skill: /skills/tidy\n\nNever rename anything.",
                    "2026-08-01T10:00:08.000Z",
                ),
                ("assistant", "Loaded.", "2026-08-01T10:00:09.000Z"),
            ],
        )],
    );

    let found = corrections(&conn).unwrap();
    let mined: Vec<(&str, Signal)> = found.iter().map(|c| (c.text.as_str(), c.signal)).collect();
    assert_eq!(
        mined,
        [(
            "don't move the vendor folder, put it back",
            Signal::Correction
        )]
    );
    assert_eq!(
        found[0].preceding.as_deref(),
        Some("Moved the shared helpers into the vendor folder.")
    );

    let replies = replies_in(&read_turns(&conn).unwrap(), &Limits::default(), 0);
    let texts: Vec<&str> = replies.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["looks good, ship it"]);
}
