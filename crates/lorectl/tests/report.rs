//! `lorectl report` over a synthetic archive and vault: literal counts per
//! section, repeats only after a note was accepted, the same numbers as
//! JSON, a narrower `--since`, and a short explanation with no archive.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn command(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lorectl"));
    command
        .args(args)
        .env("HOME", home)
        .env("LORE_VAULT_DIR", home.join("vault"))
        .env("LORE_ARCHIVE_DIR", home.join("archive"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

/// `lorectl args` as a person runs it, at a terminal.
fn person(home: &Path, args: &[&str]) -> Output {
    common::on_a_terminal(&mut command(home, args), "")
}

fn lorectl(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = command(home, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn ok(o: &Output) -> String {
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout.clone()).unwrap()
}

/// Codex sessions on 2026-08-01, -02, ..., each an opening request, then
/// `(assistant, user)`, scanned into `home/archive`.
fn scanned(home: &Path, sessions: &[(&str, &str)]) {
    for (i, (assistant, user)) in sessions.iter().enumerate() {
        let day = home.join(format!("codex/sessions/2026/08/{:02}", i + 1));
        std::fs::create_dir_all(&day).unwrap();
        let ts = |s: usize| format!("2026-08-{:02}T10:00:{s:02}.000Z", i + 1);
        let item = |s: usize, role: &str, kind: &str, text: &str| {
            serde_json::json!({"type": "response_item", "timestamp": ts(s), "payload": {
                "type": "message", "role": role, "content": [{"type": kind, "text": text}]}})
        };
        let lines = [
            serde_json::json!({"type": "session_meta", "timestamp": ts(0), "payload": {
                "id": format!("019e0000-0000-7000-8000-{:012x}", i + 1), "cwd": "/proj",
                "cli_version": "0.133.0", "model_provider": "openai"}}),
            item(1, "user", "input_text", "add the dependency"),
            item(2, "assistant", "output_text", assistant),
            item(3, "user", "input_text", user),
        ];
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(day.join(format!("rollout-{:012x}.jsonl", i + 1)), body).unwrap();
    }
    ok(&lorectl(home, &["scan"], ""));
}

const PNPM: &str = "01JD0000000000000000000001";
const GENERATED: &str = "01JD0000000000000000000002";

/// The archive, and a vault with: a pnpm note created 2026-08-02, a
/// generated-files note created 2026-08-20, three proposals (one accepted,
/// one rejected, one pending), one correction in the considered ledger, and
/// two sessions' shown records.
fn fixture(home: &Path) {
    scanned(
        home,
        &[
            ("Running npm install axios.", "no, use pnpm not npm"),
            ("I'll edit src/gen.rs.", "don't edit the generated files"),
            (
                "Running npm install lodash.",
                "wrong tool, use pnpm instead of npm",
            ),
            ("Edited src/gen.rs.", "stop editing the generated files"),
            ("npm install dayjs done.", "I told you, pnpm not npm"),
        ],
    );
    ok(&lorectl(home, &["mem", "init"], ""));
    for (name, id, title, created, body) in [
        (
            "pnpm.md",
            PNPM,
            "Use pnpm, not npm",
            "2026-08-02",
            "Install packages with pnpm; npm breaks the lockfile.",
        ),
        (
            "generated.md",
            GENERATED,
            "Leave generated files alone",
            "2026-08-20",
            "Never edit generated files; regenerate them.",
        ),
    ] {
        let path = home.join(name);
        std::fs::write(
            &path,
            format!("---\nid: {id}\ntype: fact\ntitle: {title}\ncreated: {created}\n---\n{body}\n"),
        )
        .unwrap();
        ok(&lorectl(home, &["mem", "add", path.to_str().unwrap()], ""));
    }
    let propose = |title: &str| {
        let out = ok(&lorectl(
            home,
            &[
                "mem",
                "propose",
                "--agent",
                "codex",
                "--session",
                "s1",
                "--type",
                "fact",
                "--title",
                title,
            ],
            "Run cargo clippy before every commit.\n",
        ));
        out.split_whitespace().nth(1).unwrap().to_owned()
    };
    let accepted = propose("Lint before committing");
    let rejected = propose("Lint twice");
    propose("Lint on Fridays");
    ok(&person(home, &["mem", "accept", &accepted]));
    ok(&lorectl(
        home,
        &["mem", "reject", &rejected, "--reason", "duplicate"],
        "",
    ));

    let mined: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["mem", "mine", "--json"], ""))).unwrap();
    let read = mined[0]["receipts"][0]["correction_id"].as_str().unwrap();
    let lore = home.join("vault/.lore");
    std::fs::write(
        lore.join("considered.jsonl"),
        format!("{{\"id\":\"{read}\",\"considered_at\":\"2026-08-06T00:00:00Z\",\"run\":\"r\"}}\n"),
    )
    .unwrap();
    std::fs::create_dir_all(lore.join("shown")).unwrap();
    std::fs::write(lore.join("shown/aaaa"), format!("{PNPM}\n{GENERATED}\n")).unwrap();
    std::fs::write(lore.join("shown/bbbb"), format!("{PNPM}\n")).unwrap();
}

#[test]
fn report_counts_each_section_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    fixture(home);
    let before_db = std::fs::read(home.join("archive/lore.db")).unwrap();
    let listing = |p: &Path| {
        let mut files: Vec<(String, Vec<u8>)> = walk(p)
            .into_iter()
            .map(|f| (f.display().to_string(), std::fs::read(&f).unwrap()))
            .collect();
        files.sort();
        files
    };
    let before_vault = listing(&home.join("vault"));

    let text = ok(&lorectl(home, &["report", "--since", "2026-08-01"], ""));
    let body: Vec<&str> = text.lines().skip(1).collect();
    assert!(text.starts_with("Lore report, 2026-08-01 to "), "{text}");
    assert!(text
        .lines()
        .next()
        .unwrap()
        .ends_with("(2026-08-01), all repositories"));
    assert_eq!(
        body.join("\n"),
        "\n\
         Activity, from user turns with text in the archive\n\
         \x20 5 sessions, 10 user turns\n\
         \x20 - codex: 5 sessions, 10 turns\n\
         \x20 top repositories by sessions: (no repository) 5\n\
         \n\
         Corrections, from the `lorectl mem mine` miner over sessions active in the window\n\
         \x20 5 flagged by a rule (5 correction)\n\
         \x20 0 reply candidates no rule flagged\n\
         \x20 1 of these already read by a finished Find lessons run (considered.jsonl)\n\
         \n\
         Lessons, from the vault's inbox, decisions.jsonl, and note files\n\
         \x20 3 filed, 1 accepted, 1 rejected in the window (vault-wide)\n\
         \x20 1 proposal pending now, 3 active notes in total\n\
         \n\
         Delivery, from the hooks' .lore/shown records (vault-wide)\n\
         \x20 2 sessions were shown 2 distinct notes by a hook\n\
         \x20 - Use pnpm, not npm (2 sessions)\n\
         \x20 - Leave generated files alone (1 session)\n\
         \x20 the hooks keep these records 7 days, so older deliveries are not counted\n\
         \n\
         Repeats, an estimate from word overlap between corrections and active notes\n\
         \x20 2 of 5 rule-flagged corrections with two content words or more \
         match a note accepted before them\n\
         \x20 - Use pnpm, not npm  2026-08-05  \"I told you, pnpm not npm\"\n\
         \x20 - Use pnpm, not npm  2026-08-03  \"wrong tool, use pnpm instead of npm\""
    );

    let json: serde_json::Value = serde_json::from_str(&ok(&lorectl(
        home,
        &["report", "--since", "2026-08-01", "--json"],
        "",
    )))
    .unwrap();
    assert_eq!(json["start"], "2026-08-01");
    assert_eq!(json["activity"]["sessions"], 5);
    assert_eq!(json["activity"]["user_turns"], 10);
    assert_eq!(json["activity"]["by_agent"][0]["agent"], "codex");
    assert_eq!(json["corrections"]["flagged"], 5);
    assert_eq!(json["corrections"]["replies"], 0);
    assert_eq!(json["corrections"]["already_read"], 1);
    assert_eq!(json["lessons"]["filed"], 3);
    assert_eq!(json["lessons"]["accepted"], 1);
    assert_eq!(json["lessons"]["rejected"], 1);
    assert_eq!(json["lessons"]["pending"], 1);
    assert_eq!(json["lessons"]["active_notes"], 3);
    assert_eq!(json["delivery"]["sessions"], 2);
    assert_eq!(json["delivery"]["distinct_notes"], 2);
    assert_eq!(json["delivery"]["top"][0]["sessions"], 2);
    assert_eq!(json["repeats"]["count"], 2);
    assert_eq!(json["repeats"]["checked"], 5);
    let repeated: Vec<(&str, &str)> = json["repeats"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["note_id"].as_str().unwrap(), r["date"].as_str().unwrap()))
        .collect();
    // The generated-files note was accepted after both of its corrections,
    // and the first pnpm correction came before its note.
    assert_eq!(repeated, [(PNPM, "2026-08-05"), (PNPM, "2026-08-03")]);

    assert_eq!(
        std::fs::read(home.join("archive/lore.db")).unwrap(),
        before_db
    );
    assert_eq!(
        listing(&home.join("vault")),
        before_vault,
        "report wrote to the vault"
    );
}

#[test]
fn since_narrows_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    fixture(home);
    let json: serde_json::Value = serde_json::from_str(&ok(&lorectl(
        home,
        &["--json", "report", "--since", "2026-08-04"],
        "",
    )))
    .unwrap();
    assert_eq!(json["activity"]["sessions"], 2);
    assert_eq!(json["activity"]["user_turns"], 4);
    assert_eq!(json["corrections"]["flagged"], 2);
    assert_eq!(json["repeats"]["count"], 1);
    assert_eq!(json["repeats"]["items"][0]["date"], "2026-08-05");

    // The default window is the last 7 days, long after these sessions.
    let recent: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["report", "--json"], ""))).unwrap();
    assert_eq!(recent["window"], "7d");
    assert_eq!(recent["activity"]["sessions"], 0);
    assert_eq!(recent["corrections"]["flagged"], 0);
}

#[test]
fn activity_leaves_out_sessions_a_program_started() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    scanned(home, &[("Added it.", "thanks"), ("Added it.", "thanks")]);
    // Rewrite the second rollout as a `codex exec` run and scan it again.
    let exec = home.join("codex/sessions/2026/08/02/rollout-000000000002.jsonl");
    let text = std::fs::read_to_string(&exec).unwrap();
    let text = text.replacen(
        "\"model_provider\":\"openai\"",
        "\"model_provider\":\"openai\",\"source\":\"exec\"",
        1,
    );
    assert!(text.contains("\"source\":\"exec\""), "{text}");
    std::fs::write(&exec, text).unwrap();
    ok(&lorectl(home, &["scan"], ""));

    let json: serde_json::Value = serde_json::from_str(&ok(&lorectl(
        home,
        &["--json", "report", "--since", "2026-08-01"],
        "",
    )))
    .unwrap();
    assert_eq!(json["activity"]["sessions"], 1, "{json}");
    assert_eq!(json["activity"]["user_turns"], 2, "{json}");
}

#[test]
fn no_archive_explains_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let text = ok(&lorectl(home, &["report"], ""));
    assert!(text.starts_with("No archive at "), "{text}");
    assert!(text.contains("lorectl scan"), "{text}");
    assert!(!home.join("archive").exists(), "report created an archive");
    let json: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["report", "--json"], ""))).unwrap();
    assert!(json["archive"].is_null());
    assert!(json["explanation"]
        .as_str()
        .unwrap()
        .starts_with("No archive at "));
}

#[test]
fn no_vault_reports_the_archive_and_explains() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    scanned(
        home,
        &[("Running npm install axios.", "no, use pnpm not npm")],
    );
    let text = ok(&lorectl(home, &["report", "--since", "2026-08-01"], ""));
    assert!(text.contains("  1 flagged by a rule"), "{text}");
    assert!(text.contains("\nNo vault at "), "{text}");
    assert!(!home.join("vault").exists(), "report created a vault");
}

/// A correction repeats a note only when the note was accepted before it
/// and the correction shares the note's lesson words, not just words a
/// receipt said around the lesson.
#[test]
fn repeats_need_an_earlier_note_with_the_same_lesson() {
    const FMT: &str = "01JD0000000000000000000011";
    const BRANCH: &str = "01JD0000000000000000000012";
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    scanned(
        home,
        &[
            (
                "Pushed both changes to main.",
                "no, make sure each change gets its own branch",
            ),
            ("Pushed it.", "no, run cargo fmt before you push"),
        ],
    );
    ok(&lorectl(home, &["mem", "init"], ""));
    for (name, id, title, created, body) in [
        // Accepted before both corrections. Its receipt says "make sure",
        // which the branch correction also says, about something else.
        (
            "fmt.md",
            FMT,
            "Format before pushing",
            "2026-07-01",
            "Run cargo fmt before each push.\n\nReceipts:\n\
             - codex s1 2026-06-30: \"make sure you format the code\"",
        ),
        // The branch correction's own lesson, accepted after it.
        (
            "branch.md",
            BRANCH,
            "One branch per change",
            "2026-08-10",
            "Give each change its own branch.",
        ),
    ] {
        let path = home.join(name);
        std::fs::write(
            &path,
            format!("---\nid: {id}\ntype: convention\ntitle: {title}\ncreated: {created}\n---\n{body}\n"),
        )
        .unwrap();
        ok(&lorectl(home, &["mem", "add", path.to_str().unwrap()], ""));
    }

    let json: serde_json::Value = serde_json::from_str(&ok(&lorectl(
        home,
        &["report", "--since", "2026-08-01", "--json"],
        "",
    )))
    .unwrap();
    assert_eq!(json["corrections"]["flagged"], 2, "{json}");
    assert_eq!(json["repeats"]["checked"], 2, "{json}");
    let repeated: Vec<(&str, &str)> = json["repeats"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["note_id"].as_str().unwrap(), r["date"].as_str().unwrap()))
        .collect();
    assert_eq!(repeated, [(FMT, "2026-08-02")], "{json}");
}

/// Filler is not a shared word: a correction that shares one lesson word
/// with a note, plus a contraction typed without its apostrophe that the
/// receipt also says, does not repeat it. One sharing two real lesson words
/// does.
#[test]
fn repeats_ignore_filler_shared_with_a_receipt() {
    const SNIPPET: &str = "01JD0000000000000000000021";
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    scanned(
        home,
        &[
            (
                "Updated the links.",
                "no, i dont know why you make the links so long, keep them short",
            ),
            (
                "I wrote the snippet to snippet.txt.",
                "no, paste the snippet inline in the reply",
            ),
        ],
    );
    ok(&lorectl(home, &["mem", "init"], ""));
    let path = home.join("snippet.md");
    std::fs::write(
        &path,
        format!(
            "---\nid: {SNIPPET}\ntype: convention\n\
             title: Paste the snippet inline; don't make a file for it\n\
             created: 2026-07-01\n---\n\
             Put ready-to-copy text in the reply.\n\nReceipts:\n\
             - codex s1 2026-06-30: \"dont make a file for it just paste the snippet\"\n"
        ),
    )
    .unwrap();
    ok(&lorectl(home, &["mem", "add", path.to_str().unwrap()], ""));

    let json: serde_json::Value = serde_json::from_str(&ok(&lorectl(
        home,
        &["report", "--since", "2026-08-01", "--json"],
        "",
    )))
    .unwrap();
    assert_eq!(json["repeats"]["checked"], 2, "{json}");
    let repeated: Vec<&str> = json["repeats"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["date"].as_str().unwrap())
        .collect();
    assert_eq!(repeated, ["2026-08-02"], "{json}");
}

#[test]
fn bad_arguments_are_usage_errors() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    for args in [
        &["report", "--since", "soon"][..],
        &["report", "--cwd", "/tmp", "--all-repos"][..],
        &["report", "stray"][..],
    ] {
        assert_eq!(lorectl(home, args, "").status.code(), Some(1), "{args:?}");
    }
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
