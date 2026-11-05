//! A turn that starts by invoking a skill or a slash command is the person
//! starting a workflow, not correcting the agent, and so is a turn longer
//! than the length limit.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, read_turns, Limits};
use lore_memory::skill_use::skill_usage;

#[test]
fn skill_and_command_invocations_are_not_corrections() {
    let dir = tempfile::tempdir().unwrap();
    let long = format!(
        "no, use pnpm not npm. {}",
        "Then rebuild everything. ".repeat(15)
    );
    let session = [
        ("user", "set up the repo"),
        ("assistant", "Running npm install."),
        (
            "user",
            "[$code-review](/home/me/.codex/skills/code-review/SKILL.md) don't skip tests",
        ),
        ("assistant", "Running npm install."),
        ("user", "/review no, use pnpm not npm"),
        ("assistant", "Running npm install."),
        (
            "user",
            "[$日本語](/home/me/.codex/skills/日本語/SKILL.md) don't skip tests",
        ),
        ("assistant", "Running npm install."),
        ("user", "/日本語 no, use pnpm not npm"),
        ("assistant", "Running npm install."),
        ("user", long.as_str()),
        ("assistant", "Running npm install."),
        (
            "user",
            "/home/me/app/package.json is wrong, use pnpm not npm",
        ),
        ("assistant", "Running npm install."),
        ("user", "[$50 budget] no, use pnpm not npm"),
        ("assistant", "Running npm install."),
        (
            "user",
            "do not use <command-name> tags in responses, use markdown instead",
        ),
        ("assistant", "Running npm install."),
        ("user", "no, use pnpm not npm"),
    ];
    let conn = archive::codex_archive(dir.path(), &[&session]);
    let turns = read_turns(&conn).unwrap();
    let usage = skill_usage(&conn, &turns, &Limits::default()).unwrap();
    let mut invoked_names: Vec<String> = usage.into_iter().map(|u| u.skill).collect();
    invoked_names.sort();
    assert_eq!(invoked_names, ["code-review", "review", "日本語"]);

    let texts: Vec<String> = corrections(&conn)
        .unwrap()
        .into_iter()
        .map(|c| c.text)
        .collect();
    assert_eq!(
        texts,
        [
            "/home/me/app/package.json is wrong, use pnpm not npm",
            "[$50 budget] no, use pnpm not npm",
            "do not use <command-name> tags in responses, use markdown instead",
            "no, use pnpm not npm"
        ]
    );
}
