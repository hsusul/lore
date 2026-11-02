//! `lorectl --help` end to end: the banner says what Lore is today.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::process::Command;

#[test]
fn help_banner_describes_the_archive_memory_and_skills() {
    let output = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("--help")
        .env_remove("LORE_ARCHIVE_DIR")
        .output()
        .expect("spawn lorectl");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let banner: Vec<&str> = stdout.lines().take(5).collect();
    assert_eq!(
        banner,
        [
            format!("lorectl {}", lore_core::version()).as_str(),
            "",
            "Lore keeps a local, searchable archive of your Claude Code and Codex sessions.",
            "It gives coding agents long-term memory and checks the skills they load.",
            "",
        ]
    );
    assert!(!stdout.contains("ledger of coding-agent work"));
}
