//! `lorectl mem mine` as a person runs it: lessons mined from the archive,
//! each with its receipts, and nothing written anywhere.
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

fn ok(o: &Output) -> String {
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout.clone()).unwrap()
}

/// Codex sessions on 2026-08-01, -02, ... each `(assistant, user)` pair
/// after an opening request, scanned into `home/archive`.
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
    ok(&lorectl(home, &["scan"]));
}

fn session(home: &Path, n: usize) -> String {
    let conn = lore_core::storage::open_read_only(&home.join("archive/lore.db")).unwrap();
    conn.query_row(
        "SELECT id FROM agent_session WHERE native_session_id = ?1",
        [format!("019e0000-0000-7000-8000-{n:012x}")],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn mine_prints_each_lesson_with_its_receipts_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
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
    let before = std::fs::read(home.join("archive/lore.db")).unwrap();
    let s = |n| session(home, n);
    let out = ok(&lorectl(home, &["mem", "mine", "--limit", "2"]));
    let first = out
        .lines()
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    let second = out
        .lines()
        .nth(7)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        out,
        format!(
            "2 lessons from 5 corrections. A dry run: nothing was written.\n\
             {first}  3 conversations, latest 2026-08-05\n\
             \x20   \"I told you, pnpm not npm\"\n\
             \x20   - codex {s5} 2026-08-05: \"I told you, pnpm not npm\"\n\
             \x20     after: \"npm install dayjs done.\"\n\
             \x20   - codex {s3} 2026-08-03: \"wrong tool, use pnpm instead of npm\"\n\
             \x20   - codex {s1} 2026-08-01: \"no, use pnpm not npm\"\n\
             {second}  2 conversations, latest 2026-08-04\n\
             \x20   \"stop editing the generated files\"\n\
             \x20   - codex {s4} 2026-08-04: \"stop editing the generated files\"\n\
             \x20     after: \"Edited src/gen.rs.\"\n\
             \x20   - codex {s2} 2026-08-02: \"don't edit the generated files\"\n",
            s5 = s(5),
            s4 = s(4),
            s3 = s(3),
            s2 = s(2),
            s1 = s(1),
        )
    );
    assert_eq!(std::fs::read(home.join("archive/lore.db")).unwrap(), before);
    assert!(!home.join("vault").exists(), "mine created a vault");

    let one = ok(&lorectl(home, &["mem", "mine", "--limit", "1", "--json"]));
    let json: serde_json::Value = serde_json::from_str(&one).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["id"], first.as_str());
    assert_eq!(json[0]["conversations"], 3);
    assert_eq!(json[0]["text"], "I told you, pnpm not npm");
    assert_eq!(
        json[0]["receipts"][0],
        serde_json::json!({
            "session": s(5), "agent": "codex", "date": "2026-08-05",
            "text": "I told you, pnpm not npm", "after": "npm install dayjs done.",
            "signal": "correction", "repo": null,
        })
    );
}

#[test]
fn mine_without_an_archive_says_how_to_build_one() {
    let dir = tempfile::tempdir().unwrap();
    let out = lorectl(dir.path(), &["mem", "mine"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("lorectl scan"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn mine_propose_files_a_chosen_lesson_with_its_receipts_once() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    scanned(
        home,
        &[
            ("Running npm install axios.", "no, use pnpm not npm"),
            ("I'll edit src/gen.rs.", "don't edit the generated files"),
            ("npm install dayjs done.", "I told you, pnpm not npm"),
            ("Edited src/gen.rs.", "stop editing the generated files"),
        ],
    );
    ok(&lorectl(home, &["mem", "init"]));
    let json: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["mem", "mine", "--json"]))).unwrap();
    let pnpm = json
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["text"] == "I told you, pnpm not npm")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (s1, s3) = (session(home, 1), session(home, 3));

    let out = ok(&lorectl(home, &["mem", "mine", "--propose", &pnpm]));
    let proposal = out.split_whitespace().nth(1).unwrap().to_owned();
    assert_eq!(
        out,
        format!("proposed {proposal} _inbox/codex-{s3}.jsonl for lesson {pnpm}\n")
    );
    let inbox: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["mem", "inbox", "--json"]))).unwrap();
    assert_eq!(inbox.as_array().unwrap().len(), 1);
    let item = &inbox[0];
    assert_eq!(item["agent"], "codex");
    assert_eq!(item["session"], s3.as_str());
    assert_eq!(item["source_verified"], true);
    assert_eq!(item["type"], "convention");
    assert_eq!(item["scope"], "global");
    assert_eq!(item["title"], "I told you, pnpm not npm");
    assert_eq!(
        item["body"],
        format!(
            "I told you, pnpm not npm\n\n\
             Mined by `lorectl mem mine` (lesson {pnpm}) from corrections in 2 conversations:\n\
             - 2026-08-03 codex {s3}: \"I told you, pnpm not npm\", after \"npm install dayjs done.\"\n\
             - 2026-08-01 codex {s1}: \"no, use pnpm not npm\", after \"Running npm install axios.\"\n"
        )
    );
    assert_eq!(item["receipt"]["status"], "found");

    // Proposing it again files nothing.
    assert_eq!(
        ok(&lorectl(home, &["mem", "mine", "--propose", &pnpm])),
        format!("already proposed lesson {pnpm}\n")
    );
    let again: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["mem", "inbox", "--json"]))).unwrap();
    assert_eq!(again.as_array().unwrap().len(), 1);
}

#[test]
fn mine_propose_refuses_an_unknown_lesson_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    scanned(
        home,
        &[
            ("Running npm install axios.", "no, use pnpm not npm"),
            ("npm install dayjs done.", "I told you, pnpm not npm"),
        ],
    );
    ok(&lorectl(home, &["mem", "init"]));
    let json: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["mem", "mine", "--json"]))).unwrap();
    let real = json[0]["id"].as_str().unwrap().to_owned();
    let out = lorectl(home, &["mem", "mine", "--propose", &real, "0000000000"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no lesson `0000000000`"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!home.join("vault/_inbox").exists());
}

#[test]
fn mine_takes_the_overlap_and_length_limits_from_flags() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // These two share 2 of 3 content words: 66%.
    scanned(
        home,
        &[
            ("Running npm install axios.", "no, use pnpm not npm"),
            (
                "Running npm install lodash.",
                "wrong tool, use pnpm instead of npm",
            ),
        ],
    );
    let first = |out: String| out.lines().next().unwrap().to_owned();
    assert_eq!(
        first(ok(&lorectl(home, &["mem", "mine"]))),
        "1 lesson from 2 corrections. A dry run: nothing was written."
    );
    assert_eq!(
        first(ok(&lorectl(home, &["mem", "mine", "--min-overlap", "70"]))),
        "2 lessons from 2 corrections. A dry run: nothing was written."
    );
    assert_eq!(
        first(ok(&lorectl(home, &["mem", "mine", "--max-chars", "25"]))),
        "1 lesson from 1 corrections. A dry run: nothing was written."
    );
    for bad in [
        ["--min-overlap", "0"],
        ["--min-overlap", "101"],
        ["--max-chars", "x"],
    ] {
        let out = lorectl(home, &["mem", "mine", bad[0], bad[1]]);
        assert_eq!(out.status.code(), Some(1), "{bad:?}");
    }
}

#[test]
fn mine_marks_a_lesson_a_later_session_contradicts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    scanned(
        home,
        &[
            ("Opened a PR.", "dont make a pr just push to main"),
            ("Done.", "please open a PR for the exif branch"),
        ],
    );
    let (s1, s2) = (session(home, 1), session(home, 2));
    let out = ok(&lorectl(home, &["mem", "mine"]));
    let id = out
        .lines()
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        out,
        format!(
            "1 lesson from 1 corrections. A dry run: nothing was written.\n\
             {id}  1 conversation, latest 2026-08-01, possibly outdated\n\
             \x20   \"dont make a pr just push to main\"\n\
             \x20   - codex {s1} 2026-08-01: \"dont make a pr just push to main\"\n\
             \x20     after: \"Opened a PR.\"\n\
             \x20   - possibly outdated by codex {s2} 2026-08-02: \"please open a PR for the exif branch\"\n"
        )
    );
    let json: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["mem", "mine", "--json"]))).unwrap();
    assert_eq!(
        json[0]["outdated_by"],
        serde_json::json!({
            "session": s2, "agent": "codex", "date": "2026-08-02",
            "text": "please open a PR for the exif branch",
        })
    );
}
