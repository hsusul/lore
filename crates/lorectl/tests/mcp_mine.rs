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
            item(4, "assistant", "output_text", "Understood."),
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
/// The vault is `vault` beside the archive, which need not exist.
fn mine(archive: &Path, arguments: Value) -> (Value, Value) {
    mine_pinned(archive, arguments, None)
}

/// [`mine`], pinned to the ids in `pinned` by `LORE_CANDIDATES_FILE`.
fn mine_pinned(archive: &Path, arguments: Value, pinned: Option<&Path>) -> (Value, Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lorectl"));
    command
        .arg("mcp")
        .arg("--archive")
        .arg(archive)
        .env_remove("LORE_ARCHIVE_DIR")
        .env_remove("LORE_CANDIDATES_FILE")
        .env("LORE_VAULT_DIR", archive.parent().unwrap().join("vault"));
    if let Some(pinned) = pinned {
        command.env("LORE_CANDIDATES_FILE", pinned);
    }
    let mut child = command
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
    let id = candidates[1]["receipts"][0]["correction_id"].clone();
    assert!(id.as_str().unwrap().starts_with("cr-"), "{id}");
    assert_eq!(
        candidates[1],
        json!({
            "text": "Stop asking me for permission",
            "suggested_rule": "Stop asking me for permission",
            "conversations": 1,
            "may_conflict_with": [],
            "receipts": [{
                "correction_id": id, "new": true,
                "session": session(home, 2), "agent": "codex", "date": "2026-08-02",
                "repo": null, "repo_path": null,
                "correction": "Stop asking me for permission", "before": "Shall I run it?",
                "after": "Understood.",
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
    // A first run has no archive and no vault yet: init makes both.
    assert_eq!(
        result["content"][0]["text"],
        "no archive at that path; run `lorectl init` to set Lore up, or `lorectl scan` \
         to build the archive alone"
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

#[test]
fn mine_candidates_leaves_out_one_off_orders_unless_asked_for_all() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    scanned(
        home,
        &[
            ("Running npm install.", "no, use pnpm not npm"),
            (
                "Opened a PR.",
                "no, never merge https://github.com/acme/app/pull/478 push fix/exif-478 instead",
            ),
        ],
    );
    let texts = |reply: &Value| -> Vec<String> {
        reply["structuredContent"]["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["text"].as_str().unwrap().to_owned())
            .collect()
    };
    let (tool, default) = mine(&home.join("archive"), json!({}));
    assert_eq!(texts(&default), ["no, use pnpm not npm"]);
    assert_eq!(default["structuredContent"]["total"], 1);
    assert_eq!(default["structuredContent"]["left_out"], 1);
    assert_eq!(tool["inputSchema"]["properties"]["all"]["type"], "boolean");
    let (_, all) = mine(&home.join("archive"), json!({ "all": true }));
    assert_eq!(
        texts(&all),
        [
            "no, use pnpm not npm",
            "no, never merge https://github.com/acme/app/pull/478 push fix/exif-478 instead"
        ]
    );
    assert_eq!(all["structuredContent"]["left_out"], 0);
}

/// One Codex session per `(cwd, agent action, user turn)`, scanned.
fn scanned_in(home: &Path, sessions: &[(&Path, &str, &str)]) {
    for (i, (cwd, assistant, user)) in sessions.iter().enumerate() {
        let day = home.join(format!("codex/sessions/2026/08/{:02}", i + 1));
        std::fs::create_dir_all(&day).unwrap();
        let ts = |s: usize| format!("2026-08-{:02}T10:00:{s:02}.000Z", i + 1);
        let item = |s: usize, role: &str, kind: &str, text: &str| {
            json!({"type": "response_item", "timestamp": ts(s), "payload": {
                "type": "message", "role": role, "content": [{"type": kind, "text": text}]}})
        };
        let lines = [
            json!({"type": "session_meta", "timestamp": ts(0), "payload": {
                "id": format!("019e0000-0000-7000-8000-{:012x}", i + 1), "cwd": cwd,
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

#[test]
fn mine_candidates_names_each_receipts_repository_and_the_cwd_repository() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let repo = home.join("app");
    std::fs::create_dir_all(&repo).unwrap();
    for args in [
        &["init", "-q"][..],
        &["remote", "add", "origin", "git@github.com:hsusul/lore.git"],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
    }
    let elsewhere = home.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    scanned_in(
        home,
        &[
            (&repo, "Running npm install.", "no, use pnpm not npm"),
            (
                &elsewhere,
                "Editing src/generated/schema.rs.",
                "never hand edit generated files",
            ),
        ],
    );
    let lore = json!({ "name": "lore", "scope": "repo:81444c90f1ff44ac" });
    let receipt_repos = |reply: &Value| -> Vec<(String, Value)> {
        reply["structuredContent"]["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["text"].as_str().unwrap().to_owned(),
                    c["receipts"][0]["repo"].clone(),
                )
            })
            .collect()
    };
    let (tool, here) = mine(&home.join("archive"), json!({ "cwd": repo }));
    assert_eq!(tool["inputSchema"]["properties"]["cwd"]["type"], "string");
    assert_eq!(here["structuredContent"]["cwd_repo"], lore);
    let mut repos = receipt_repos(&here);
    repos.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        repos,
        [
            ("never hand edit generated files".to_owned(), Value::Null),
            ("no, use pnpm not npm".to_owned(), lore),
        ]
    );
    let (_, anywhere) = mine(&home.join("archive"), json!({}));
    assert_eq!(anywhere["structuredContent"]["cwd_repo"], Value::Null);
}

/// `(correction_id, new)` of each receipt of each candidate in a reply.
fn marks(result: &Value) -> Vec<Vec<(String, bool)>> {
    result["structuredContent"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            c["receipts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    let id = r["correction_id"].as_str().unwrap().to_owned();
                    (id, r["new"].as_bool().unwrap())
                })
                .collect()
        })
        .collect()
}

fn pnpm_and_permission(home: &Path) {
    scanned(
        home,
        &[
            ("Running npm install.", "no, use pnpm not npm"),
            ("Shall I run it?", "Stop asking me for permission"),
            ("Running npm install lodash.", "I told you, pnpm not npm"),
        ],
    );
}

fn ids(texts: &[&String]) -> Vec<lore_memory::considered::CorrectionId> {
    texts
        .iter()
        .map(|t| lore_memory::considered::CorrectionId::parse(t).unwrap())
        .collect()
}

#[test]
fn mine_candidates_lists_only_lessons_with_a_receipt_no_run_has_read() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    pnpm_and_permission(home);
    let archive = home.join("archive");
    let (tool, first) = mine(&archive, json!({}));
    assert!(
        tool["description"].as_str().unwrap().contains("all_seen"),
        "{tool}"
    );
    assert!(tool["inputSchema"]["properties"]["all_seen"].is_object());
    let before = marks(&first);
    let (pnpm, permission) = (&before[0], &before[1]);
    assert!(before.iter().flatten().all(|(_, new)| *new));
    assert_eq!(first["structuredContent"]["already_considered"], 0);

    // A run read the older pnpm receipt only: the lesson stays, marked.
    let (vault, _) = lore_memory::vault::Vault::init(&home.join("vault")).unwrap();
    let older = ids(&[&pnpm[1].0]);
    lore_memory::considered::record(&vault, &older, "run-1").unwrap();
    let (_, partly) = mine(&archive, json!({}));
    assert_eq!(
        marks(&partly),
        [
            vec![(pnpm[0].0.clone(), true), (pnpm[1].0.clone(), false)],
            permission.clone()
        ]
    );

    // Every pnpm receipt read: only the permission lesson is new.
    lore_memory::considered::record(&vault, &ids(&[&pnpm[0].0]), "run-2").unwrap();
    let (_, after) = mine(&archive, json!({}));
    assert_eq!(marks(&after), std::slice::from_ref(permission));
    assert_eq!(after["structuredContent"]["total"], 1);
    assert_eq!(after["structuredContent"]["already_considered"], 1);
    let (_, everything) = mine(&archive, json!({ "all_seen": true }));
    assert_eq!(
        marks(&everything),
        [
            vec![(pnpm[0].0.clone(), false), (pnpm[1].0.clone(), false)],
            permission.clone()
        ]
    );
}

#[test]
fn mine_candidates_shows_every_new_receipt_and_caps_only_read_ones() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let same = vec![("Running npm install.", "no, use pnpm not npm"); 8];
    scanned(home, &same);
    let archive = home.join("archive");
    let (_, first) = mine(&archive, json!({}));
    let all = &marks(&first)[0];
    assert_eq!(
        all.len(),
        8,
        "every new receipt is shown, past the cap of 5"
    );

    // Seven read: the new one plus five of the read ones, as context.
    let (vault, _) = lore_memory::vault::Vault::init(&home.join("vault")).unwrap();
    let read: Vec<&String> = all[1..].iter().map(|(id, _)| id).collect();
    lore_memory::considered::record(&vault, &ids(&read), "run-1").unwrap();
    let (_, later) = mine(&archive, json!({}));
    let shown = &marks(&later)[0];
    assert_eq!(shown.iter().filter(|(_, new)| *new).count(), 1);
    assert_eq!(shown.iter().filter(|(_, new)| !*new).count(), 5);
}

#[test]
fn mine_candidates_serves_exactly_the_pinned_corrections() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    pnpm_and_permission(home);
    let archive = home.join("archive");
    let (_, first) = mine(&archive, json!({}));
    let before = marks(&first);
    // The ledger says the pnpm lesson was read; the pin ignores it.
    let (vault, _) = lore_memory::vault::Vault::init(&home.join("vault")).unwrap();
    let pnpm_ids = ids(&[&before[0][0].0, &before[0][1].0]);
    lore_memory::considered::record(&vault, &pnpm_ids, "run-1").unwrap();
    let pin = home.join("pinned.txt");
    std::fs::write(&pin, format!("{}\n", before[0][1].0)).unwrap();
    let (_, pinned) = mine_pinned(&archive, json!({ "all_seen": true }), Some(&pin));
    assert_eq!(
        marks(&pinned),
        [vec![
            (before[0][0].0.clone(), false),
            (before[0][1].0.clone(), true)
        ]]
    );
    assert_eq!(pinned["structuredContent"]["not_in_this_run"], 1);
    assert!(pinned["structuredContent"]["already_considered"].is_null());
    std::fs::write(&pin, "").unwrap();
    let (_, none) = mine_pinned(&archive, json!({}), Some(&pin));
    assert_eq!(none["structuredContent"]["total"], 0);
}
