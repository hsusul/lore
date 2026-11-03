//! `lorectl init` as a person runs it the first time, and again: the vault,
//! a scan of recent sessions, and the lessons in them, then what to do next.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

fn lorectl(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .env("HOME", home)
        .env("LORE_VAULT_DIR", home.join("vault"))
        .env("LORE_ARCHIVE_DIR", home.join("archive"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

/// Stdout of a run that exited 0, with the elapsed seconds masked as `N`.
fn masked(o: &Output) -> String {
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let text = String::from_utf8(o.stdout.clone()).unwrap();
    text.lines()
        .map(|line| match line.rsplit_once(" in ") {
            Some((head, tail)) if line.starts_with("Archive") && tail.ends_with(" s") => {
                format!("{head} in N s\n")
            }
            _ => format!("{line}\n"),
        })
        .collect()
}

/// One Claude Code session on `day`: a request, the agent's reply, and the
/// person's correction.
fn claude_session(home: &Path, n: u32, day: &str, reply: &str, correction: &str) {
    let project = home.join("claude/projects/-proj");
    std::fs::create_dir_all(&project).unwrap();
    let session = format!("5e550000-0000-4000-8000-{n:012x}");
    let line = |i: u32, kind: &str, parent: Option<String>, message: serde_json::Value| {
        serde_json::json!({
            "type": kind, "uuid": format!("{kind}-{n}-{i}"), "parentUuid": parent,
            "sessionId": session, "cwd": "/proj",
            "timestamp": format!("{day}T10:00:0{i}.000Z"), "message": message,
        })
    };
    let lines = [
        line(
            0,
            "user",
            None,
            serde_json::json!({"role": "user", "content": "tidy up the repository"}),
        ),
        line(
            1,
            "assistant",
            Some(format!("user-{n}-0")),
            serde_json::json!({"role": "assistant",
                "content": [{"type": "text", "text": reply}]}),
        ),
        line(
            2,
            "user",
            Some(format!("assistant-{n}-1")),
            serde_json::json!({"role": "user", "content": correction}),
        ),
    ];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(project.join(format!("{session}.jsonl")), body).unwrap();
}

fn archived_id(home: &Path, n: u32) -> String {
    let conn = lore_core::storage::open_read_only(&home.join("archive/lore.db")).unwrap();
    conn.query_row(
        "SELECT id FROM agent_session WHERE native_session_id = ?1",
        [format!("5e550000-0000-4000-8000-{n:012x}")],
        |r| r.get(0),
    )
    .unwrap()
}

const NEXT: &str = "\
Next
  Keep one:       lorectl mem mine --propose <id>, then lorectl mem accept
  Better wording: ask Claude Code or Codex to call mine_candidates and propose lessons
  Connect agents: lorectl connect
";

#[test]
fn init_creates_the_vault_scans_and_shows_the_lesson_then_reuses_both() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    claude_session(
        home,
        1,
        "2026-10-20",
        "Added docs/ to the index.",
        "no, docs are never tracked in git",
    );
    claude_session(
        home,
        2,
        "2026-10-28",
        "Staged docs/notes.md with the rest.",
        "I told you, docs are never tracked in git",
    );
    // A one-off task order, mined as a lesson by `mem mine` but with a rule
    // score below 0 for the branch it names, so init leaves it out.
    for (n, day) in [(3, "2026-10-25"), (4, "2026-10-26")] {
        claude_session(
            home,
            n,
            day,
            "Pushed to main.",
            "no, push to the feat/login-fix branch",
        );
    }

    let first = masked(&lorectl(home, &["init"]));
    let id = first
        .lines()
        .find(|l| l.contains("2 conversations"))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("<missing>")
        .to_owned();
    let lessons = format!(
        "Lessons    1 candidate from your corrections:\n\
         \n\
         \x20 {id}  2 conversations, latest 2026-10-28\n\
         \x20     \"I told you, docs are never tracked in git\"\n\
         \x20     - claude-code {s2} 2026-10-28: \"I told you, docs are never tracked in git\"\n\
         \n",
        s2 = archived_id(home, 2),
    );
    let vault = home.join("vault");
    assert_eq!(
        first,
        format!(
            "Lore\n\
             Vault      created {vault}\n\
             Archive    scanned 4 new sessions from the last 30d in N s\n\
             {lessons}{NEXT}",
            vault = vault.display()
        )
    );
    assert!(vault.join(".git").exists(), "init made no vault");
    let mined = String::from_utf8(lorectl(home, &["mem", "mine"]).stdout).unwrap();
    assert!(mined.starts_with("2 lessons"), "{mined}");

    let second = masked(&lorectl(home, &["init", "--since", "7d", "--limit", "1"]));
    assert_eq!(
        second,
        format!(
            "Lore\n\
             Vault      {vault}, 0 notes\n\
             Archive    no new sessions from the last 7d in N s\n\
             {lessons}{NEXT}",
            vault = vault.display()
        )
    );
}

#[test]
fn init_on_a_home_with_no_sessions_says_there_is_nothing_yet() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let out = masked(&lorectl(home, &["init"]));
    assert_eq!(
        out,
        format!(
            "Lore\n\
             Vault      created {vault}\n\
             Archive    no new sessions from the last 30d in N s\n\
             Lessons    none found in your sessions yet.\n\
             \n\
             {NEXT}",
            vault = home.join("vault").display()
        )
    );
}

#[test]
fn init_refuses_a_bad_limit_a_bad_window_and_a_stray_operand() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &["init", "--limit", "0"][..],
        &["init", "--since", "soon"],
        &["init", "stray"],
    ] {
        let out = lorectl(dir.path(), args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
    }
    assert!(
        !dir.path().join("vault").exists(),
        "a usage error wrote a vault"
    );
}
