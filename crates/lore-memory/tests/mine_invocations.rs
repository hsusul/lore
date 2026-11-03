//! A turn that starts by invoking a skill or a slash command is the person
//! starting a workflow, not correcting the agent, and so is a turn longer
//! than the length limit.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::corrections;

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
        ("user", long.as_str()),
        ("assistant", "Running npm install."),
        (
            "user",
            "/home/me/app/package.json is wrong, use pnpm not npm",
        ),
        ("assistant", "Running npm install."),
        ("user", "no, use pnpm not npm"),
    ];
    let conn = archive::codex_archive(dir.path(), &[&session]);
    let texts: Vec<String> = corrections(&conn)
        .unwrap()
        .into_iter()
        .map(|c| c.text)
        .collect();
    assert_eq!(
        texts,
        [
            "/home/me/app/package.json is wrong, use pnpm not npm",
            "no, use pnpm not npm"
        ]
    );
}
