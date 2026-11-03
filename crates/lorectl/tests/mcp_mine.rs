//! The `mine_candidates` MCP tool: the miner's candidate lessons, newest
//! first, each with receipts, for the agent to group by meaning and file with
//! `propose_memory`. It reads the archive and writes nothing.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const TOKEN: &str = concat!("ghp_", "Zq7Lm2Xc9Vb4Nk8Rt1Yw6Pd3Fs5Gh0Jk2Lm4Q");

/// Codex sessions, each `(agent action, user turn)` after an opening request,
/// on 2026-08-01, -02, ..., scanned into `home/archive`.
fn scanned(home: &Path, sessions: &[(&str, &str)]) {
    for (i, (assistant, user)) in sessions.iter().enumerate() {
        let (month, date) = (8 + i / 28, i % 28 + 1);
        let day = home.join(format!("codex/sessions/2026/{month:02}/{date:02}"));
        std::fs::create_dir_all(&day).unwrap();
        let ts = |s: usize| format!("2026-{month:02}-{date:02}T10:00:{s:02}.000Z");
        let item = |s: usize, role: &str, kind: &str, text: &str| {
            json!({"type": "response_item", "timestamp": ts(s), "payload": {
                "type": "message", "role": role, "content": [{"type": kind, "text": text}]}})
        };
        let lines = [
            json!({"type": "session_meta", "timestamp": ts(0), "payload": {
                "id": format!("019e0000-0000-7000-8000-{:012x}", i + 1), "cwd": "/proj",
                "cli_version": "0.133.0", "model_provider": "openai"}}),
            item(1, "user", "input_text", "go ahead"),
            item(2, "assistant", "output_text", assistant),
            item(3, "user", "input_text", user),
        ];
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(day.join(format!("rollout-{:012x}.jsonl", i + 1)), body).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["scan", "--archive"])
        .arg(home.join("archive"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .env_remove("LORE_ARCHIVE_DIR")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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

/// Sixty corrections that share no words, each with an agent turn long
/// enough that they cannot all fit in one reply.
fn sixty(home: &Path) {
    let filler = "and keep the rest of the module exactly as it was before. ".repeat(3);
    let texts: Vec<(String, String)> = (0..60)
        .map(|n| {
            let word = |k: usize| format!("w{n}x{k}");
            let words: Vec<String> = (1..=12).map(word).collect();
            (
                format!("Editing {} now, {filler}", word(0)),
                format!("never touch {}", words.join(" ")),
            )
        })
        .collect();
    let pairs: Vec<(&str, &str)> = texts
        .iter()
        .map(|(a, u)| (a.as_str(), u.as_str()))
        .collect();
    scanned(home, &pairs);
}

#[test]
fn mine_candidates_pages_through_every_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    sixty(home);
    let (tool, _) = mine(&home.join("archive"), json!({}));
    let description = tool["description"].as_str().unwrap();
    assert!(description.contains("next_cursor"), "{description}");
    assert!(description.contains("every page"), "{description}");

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut arguments = json!({ "limit": 50 });
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let (_, result) = mine(&home.join("archive"), arguments);
        let reply = &result["structuredContent"];
        assert!(
            reply.to_string().chars().count() <= 16_000,
            "page {pages} too big"
        );
        assert_eq!(reply["total"], 60);
        for candidate in reply["candidates"].as_array().unwrap() {
            seen.push(candidate["text"].as_str().unwrap().to_owned());
        }
        pages += 1;
        match reply["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert!(pages > 1, "sixty long candidates fit in one page");
    let distinct: std::collections::BTreeSet<&String> = seen.iter().collect();
    assert_eq!((seen.len(), distinct.len()), (60, 60));

    let (_, bad) = mine(&home.join("archive"), json!({ "cursor": "x" }));
    assert_eq!(bad["isError"], true);
    assert_eq!(bad["structuredContent"]["field"], "cursor");
}

/// One `mine_candidates` call over stdio, and the `tools/list` entry for it.
fn mine(archive: &Path, arguments: Value) -> (Value, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("mcp")
        .arg("--archive")
        .arg(archive)
        .env_remove("LORE_ARCHIVE_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    for message in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "mine_candidates", "arguments": arguments}}),
    ] {
        writeln!(pipe, "{message}").unwrap();
    }
    drop(pipe);
    let out = child.wait_with_output().unwrap();
    let replies: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let tool = replies[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "mine_candidates")
        .cloned()
        .unwrap();
    (tool, replies[1]["result"].clone())
}

#[test]
fn mine_candidates_returns_receipts_newest_first_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    scanned(
        home,
        &[
            ("Running npm install.", "no, use pnpm not npm"),
            ("Shall I run it?", "Stop asking me for permission"),
            (
                &format!("Running npm install with {TOKEN}."),
                "I told you, pnpm not npm",
            ),
        ],
    );
    let (tool, result) = mine(&home.join("archive"), json!({}));
    assert!(
        tool["description"]
            .as_str()
            .unwrap()
            .contains("propose_memory"),
        "{tool}"
    );
    let reply = &result["structuredContent"];
    let text = reply.to_string();
    assert!(!text.contains(TOKEN), "{text}");
    let candidates = reply["candidates"].as_array().unwrap();
    let summary: Vec<(u64, Vec<String>)> = candidates
        .iter()
        .map(|c| {
            let sessions = c["receipts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["session"].as_str().unwrap().to_owned())
                .collect();
            (c["conversations"].as_u64().unwrap(), sessions)
        })
        .collect();
    assert_eq!(
        summary,
        [
            (2, vec![session(home, 3), session(home, 1)]),
            (1, vec![session(home, 2)])
        ]
    );
    assert_eq!(
        candidates[1],
        json!({
            "text": "Stop asking me for permission",
            "suggested_rule": "Stop asking me for permission",
            "conversations": 1,
            "may_conflict_with": [],
            "receipts": [{
                "session": session(home, 2), "agent": "codex", "date": "2026-08-02",
                "correction": "Stop asking me for permission", "before": "Shall I run it?",
            }],
        })
    );
    assert_eq!(reply["next_cursor"], Value::Null);
    assert_eq!(reply["total"], 2);

    let (_, one) = mine(&home.join("archive"), json!({ "limit": 1 }));
    assert_eq!(
        one["structuredContent"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn mine_candidates_suggests_a_rule_in_the_persons_own_words() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let said = "yes do that, also make sure that you commit and push every little step";
    scanned(home, &[("Committed the parser. Want me to push?", said)]);
    let (_, result) = mine(&home.join("archive"), json!({}));
    let candidate = &result["structuredContent"]["candidates"][0];
    assert_eq!(candidate["text"], said);
    assert_eq!(
        candidate["suggested_rule"],
        "Commit and push every little step"
    );
}

#[test]
fn mine_candidates_without_an_archive_is_a_tool_error() {
    let dir = tempfile::tempdir().unwrap();
    let (_, result) = mine(&dir.path().join("none"), json!({}));
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("lorectl scan"),
        "{result}"
    );
}

#[test]
fn mine_candidates_lists_later_turns_that_may_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    scanned(
        home,
        &[
            ("Opened a PR.", "dont make a pr just push to main"),
            ("Done.", "please open a PR for the exif branch"),
        ],
    );
    let (_, result) = mine(&home.join("archive"), json!({}));
    let candidate = &result["structuredContent"]["candidates"][0];
    assert_eq!(
        candidate["may_conflict_with"],
        json!([{
            "session": session(home, 2), "agent": "codex", "date": "2026-08-02",
            "said": "please open a PR for the exif branch",
        }])
    );
}
