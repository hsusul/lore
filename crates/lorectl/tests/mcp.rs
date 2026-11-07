//! `lorectl mcp` end to end: a real process spoken to over stdin/stdout, the
//! way an agent launches it.
//!
//! Agent homes are isolated per child process; no real `~/.claude` or `~/.codex`
//! history is read (`docs/development/TESTING.md`).
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const SECRET_BODY: &str = "0123456789abcdefghijklmnopqrstuvwxyz";
const SESSION_UUID: &str = "11111111-2222-3333-4444-555555555555";

fn lorectl(archive: &Path, homes: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .arg("--archive")
        .arg(archive)
        .env("CLAUDE_CONFIG_DIR", homes.join("claude"))
        .env("CODEX_HOME", homes.join("codex"))
        .env_remove("LORE_ARCHIVE_DIR")
        .output()
        .expect("spawn lorectl")
}

/// Send `messages` to `lorectl mcp`, close stdin, and return every reply.
fn converse(archive: &Path, messages: &[Value]) -> Vec<Value> {
    converse_with(archive, &[], messages)
}

/// [`converse`] with extra environment variables set on the server.
fn converse_with(archive: &Path, envs: &[(&str, &str)], messages: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("mcp")
        .arg("--archive")
        .arg(archive)
        .env_remove("LORE_ARCHIVE_DIR")
        .env_remove("LORE_AGENT")
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lorectl mcp");
    {
        let mut stdin = child.stdin.take().unwrap();
        for message in messages {
            writeln!(stdin, "{message}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "server exits 0 when stdin closes"
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("every stdout line is JSON"))
        .collect()
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn call(id: i64, tool: &str, arguments: Value) -> Value {
    request(
        id,
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    )
}

/// One Claude Code session: a user turn carrying a secret, then `replies`
/// numbered assistant turns.
fn plant_session(homes: &Path, replies: usize) {
    let project = homes.join("claude/projects/p");
    std::fs::create_dir_all(&project).unwrap();
    let mut jsonl = format!(
        "{{\"type\":\"user\",\"uuid\":\"u0\",\"sessionId\":\"{SESSION_UUID}\",\"cwd\":\"/p\",\
\"message\":{{\"role\":\"user\",\"content\":\"deploy the zanzibar service with token ghp_{SECRET_BODY}\"}}}}\n"
    );
    let mut parent = "u0".to_string();
    for i in 0..replies {
        let uuid = format!("a{i}");
        jsonl.push_str(&format!(
            "{{\"type\":\"assistant\",\"uuid\":\"{uuid}\",\"parentUuid\":\"{parent}\",\
\"sessionId\":\"{SESSION_UUID}\",\"cwd\":\"/p\",\"message\":{{\"role\":\"assistant\",\
\"content\":[{{\"type\":\"text\",\"text\":\"step {i}\"}}]}}}}\n"
        ));
        parent = uuid;
    }
    std::fs::write(project.join(format!("{SESSION_UUID}.jsonl")), jsonl).unwrap();
}

fn scanned_archive(replies: usize) -> (tempfile::TempDir, tempfile::TempDir) {
    let archive = tempfile::tempdir().unwrap();
    let homes = common::empty_homes();
    plant_session(homes.path(), replies);
    let out = lorectl(archive.path(), homes.path(), &["scan"]);
    assert_eq!(out.status.code(), Some(0), "seeding scan failed");
    (archive, homes)
}

fn only_session_id(archive: &Path) -> String {
    let conn = lore_core::storage::open_read_only(&common::db_path(archive)).unwrap();
    conn.query_row("SELECT id FROM agent_session", [], |r| r.get(0))
        .unwrap()
}

fn structured(reply: &Value) -> &Value {
    assert_eq!(reply["result"].get("isError"), None, "{reply}");
    &reply["result"]["structuredContent"]
}

#[test]
fn speaks_the_mcp_handshake() {
    let archive = tempfile::tempdir().unwrap();
    let replies = converse(
        archive.path(),
        &[
            request(1, "initialize", json!({ "protocolVersion": "2025-03-26" })),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            request(2, "tools/list", json!({})),
            request(3, "ping", json!({})),
            request(4, "resources/list", json!({})),
            json!("not a request"),
        ],
    );

    assert_eq!(
        replies.len(),
        5,
        "the notification gets no reply: {replies:?}"
    );
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "lorectl");
    let names: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "search_sessions",
            "read_session",
            "recall_memory",
            "read_note",
            "mine_candidates",
            "propose_memory"
        ]
    );
    assert_eq!(
        replies[2],
        json!({ "jsonrpc": "2.0", "id": 3, "result": {} })
    );
    assert_eq!(replies[3]["error"]["code"], -32601);
    assert_eq!(replies[4]["error"]["code"], -32600);
}

#[test]
fn propose_memory_points_session_id_at_the_session_start_line() {
    let archive = tempfile::tempdir().unwrap();
    let replies = converse(archive.path(), &[request(1, "tools/list", json!({}))]);
    let propose = replies[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "propose_memory")
        .unwrap();
    assert_eq!(
        propose["inputSchema"]["properties"]["session_id"]["description"],
        "The id after `Lore session:` in your session-start context, else \
         your own session id, if known."
    );
}

#[test]
fn an_unknown_protocol_version_is_answered_with_the_newest() {
    let archive = tempfile::tempdir().unwrap();
    let replies = converse(
        archive.path(),
        &[request(
            1,
            "initialize",
            json!({ "protocolVersion": "1999-01-01" }),
        )],
    );
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn a_missing_archive_is_a_tool_error_not_a_dead_server() {
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("not-yet");
    let replies = converse(
        &missing,
        &[
            call(1, "search_sessions", json!({ "query": "anything" })),
            request(2, "ping", json!({})),
        ],
    );
    assert_eq!(replies[0]["result"]["isError"], true);
    let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("lorectl scan"), "names the fix: {text}");
    assert_eq!(
        replies[1]["result"],
        json!({}),
        "still serving after the error"
    );
    assert!(!missing.exists(), "the server created an archive");
}

#[test]
fn search_then_read_finds_the_session_without_its_secret() {
    let (archive, _homes) = scanned_archive(2);
    let replies = converse(
        archive.path(),
        &[
            call(1, "search_sessions", json!({ "query": "zanzibar" })),
            call(2, "search_sessions", json!({ "query": "nonexistentterm" })),
        ],
    );
    let hits = structured(&replies[0])["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    let session_id = hits[0]["session_id"].as_str().unwrap().to_owned();
    let snippet = hits[0]["snippet"].as_str().unwrap();
    assert!(snippet.contains("zanzibar"), "{snippet}");
    assert_eq!(structured(&replies[1])["hits"], json!([]));

    let replies = [
        replies,
        converse(
            archive.path(),
            &[call(1, "read_session", json!({ "session_id": session_id }))],
        ),
    ]
    .concat();
    let read = structured(&replies[2]);
    assert_eq!(read["session_id"], session_id);
    let entries: Vec<(&str, &str)> = read["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["label"].as_str().unwrap(), e["text"].as_str().unwrap()))
        .collect();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[1], ("assistant", "step 0"));
    assert_eq!(entries[2], ("assistant", "step 1"));
    assert_eq!(entries[0].0, "user");
    assert!(entries[0]
        .1
        .starts_with("deploy the zanzibar service with token "));
    assert_eq!(read["next_cursor"], Value::Null);

    for reply in &replies {
        assert!(
            !reply.to_string().contains(SECRET_BODY),
            "a secret left the archive: {reply}"
        );
    }
}

#[test]
fn read_session_pages_until_the_cursor_runs_out() {
    let (archive, _homes) = scanned_archive(60);
    let session_id = only_session_id(archive.path());
    let first = converse(
        archive.path(),
        &[call(1, "read_session", json!({ "session_id": session_id }))],
    );
    let page = structured(&first[0]);
    assert_eq!(page["entries"].as_array().unwrap().len(), 50);
    let cursor = page["next_cursor"]
        .as_str()
        .expect("a full page has a cursor");

    let second = converse(
        archive.path(),
        &[call(
            1,
            "read_session",
            json!({ "session_id": session_id, "cursor": cursor }),
        )],
    );
    let page = structured(&second[0]);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 11, "60 replies plus the user turn, minus 50");
    assert_eq!(entries[10]["text"], "step 59");
    assert_eq!(page["next_cursor"], Value::Null);
}

#[test]
fn unknown_sessions_and_tools_are_reported() {
    let (archive, _homes) = scanned_archive(1);
    let replies = converse(
        archive.path(),
        &[
            call(1, "read_session", json!({ "session_id": "nope" })),
            call(2, "search_sessions", json!({})),
            call(3, "delete_everything", json!({})),
        ],
    );
    assert_eq!(replies[0]["result"]["isError"], true);
    assert_eq!(
        replies[0]["result"]["content"][0]["text"],
        "no session `nope` in the archive"
    );
    assert_eq!(
        replies[1]["result"]["content"][0]["text"],
        "`query` is required"
    );
    assert_eq!(replies[2]["error"]["code"], -32602);
}

const GIT_ENV: [(&str, &str); 2] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
];

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=T", "-c", "user.email=t@localhost"])
        .args(args)
        .envs(GIT_ENV)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `lorectl mem <args>` against `<home>/vault`, with `HOME` at `home`.
/// Returns stdout.
fn mem(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("mem")
        .args(args)
        .arg("--vault")
        .arg(home.join("vault"))
        .envs(GIT_ENV)
        .env("HOME", home)
        .env_remove("LORE_VAULT_DIR")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A temporary `HOME` holding an initialized vault at `<home>/vault`.
fn memory_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    mem(home.path(), &["init"]);
    home
}

/// `mem add` a note written to a file in `home`, with extra `args`.
fn add_note(home: &Path, text: &str, args: &[&str]) {
    let file = home.join("note.md");
    std::fs::write(&file, text).unwrap();
    mem(home, &[&["add", file.to_str().unwrap()], args].concat());
}

/// One `recall_memory` call, with `HOME` at `home` and the vault at `vault`.
fn recall(home: &Path, vault: &Path, arguments: Value) -> Value {
    memory_tool(home, vault, "recall_memory", arguments)
}

/// One `read_note` call, with `HOME` at `home` and the vault at `vault`.
fn read_note(home: &Path, vault: &Path, arguments: Value) -> Value {
    memory_tool(home, vault, "read_note", arguments)
}

/// One call to a memory `tool`, with `HOME` at `home` and the vault at
/// `vault`. Checks the text content carries the same JSON.
fn memory_tool(home: &Path, vault: &Path, tool: &str, arguments: Value) -> Value {
    let envs = [
        GIT_ENV[0],
        GIT_ENV[1],
        ("HOME", home.to_str().unwrap()),
        ("LORE_VAULT_DIR", vault.to_str().unwrap()),
    ];
    let mut replies = converse_with(&home.join("archive"), &envs, &[call(1, tool, arguments)]);
    let reply = replies.remove(0);
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        reply["result"]["structuredContent"],
        "the text content carries the same JSON"
    );
    reply
}

const REPO_NOTE: &str = "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H6D\ntype: pitfall\n\
title: Retry the migration\nscope: repo:81444c90f1ff44ac\n\
anchors:\n  - path: src/a.rs\n    lines: 1\n---\nRetry a failed migration once.\n";

#[test]
fn recall_memory_labels_a_fresh_note_and_returns_no_notes_for_no_match() {
    let home = memory_home();
    let repo = home.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:hsusul/lore.git"],
    );
    std::fs::write(repo.join("src/a.rs"), "fn retry() {}\n").unwrap();
    let cwd = repo.to_str().unwrap();
    add_note(home.path(), REPO_NOTE, &["--cwd", cwd]);
    let vault = home.path().join("vault");

    let reply = recall(
        home.path(),
        &vault,
        json!({ "cwd": cwd, "query": "migration" }),
    );

    assert_eq!(
        structured(&reply),
        &json!({
            "vault": "ok",
            "notes": [{
                "id": "01JC9V6Q4M8K2T7X3N5R0B1H6D",
                "title": "Retry the migration",
                "scope": "repo:81444c90f1ff44ac",
                "freshness": "fresh",
                "excerpt": "Retry a failed migration once.",
            }],
            "total": 1,
            "truncated": false,
        })
    );

    let reply = recall(
        home.path(),
        &vault,
        json!({ "cwd": cwd, "query": "kingfisher" }),
    );
    assert_eq!(
        structured(&reply),
        &json!({ "vault": "ok", "notes": [], "total": 1, "truncated": false })
    );
}

#[test]
fn recall_memory_without_words_returns_every_note_in_scope() {
    let home = memory_home();
    let repo = home.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:hsusul/lore.git"],
    );
    std::fs::write(repo.join("src/a.rs"), "fn retry() {}\n").unwrap();
    let cwd = repo.to_str().unwrap();
    add_note(home.path(), REPO_NOTE, &["--cwd", cwd]);
    let vault = home.path().join("vault");
    let expected = json!({
        "vault": "ok",
        "notes": [{
            "id": "01JC9V6Q4M8K2T7X3N5R0B1H6D",
            "title": "Retry the migration",
            "scope": "repo:81444c90f1ff44ac",
            "freshness": "fresh",
            "excerpt": "Retry a failed migration once.",
        }],
        "total": 1,
        "truncated": false,
    });

    for arguments in [
        json!({ "cwd": cwd }),
        json!({ "cwd": cwd, "query": "" }),
        json!({ "cwd": cwd, "query": "--" }),
    ] {
        let reply = recall(home.path(), &vault, arguments.clone());
        assert_eq!(structured(&reply), &expected, "{arguments}");
    }
}

#[test]
fn recall_memory_schema_requires_only_cwd() {
    let archive = tempfile::tempdir().unwrap();
    let replies = converse(archive.path(), &[request(1, "tools/list", json!({}))]);
    let recall = replies[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "recall_memory")
        .unwrap();
    assert_eq!(recall["inputSchema"]["required"], json!(["cwd"]));
    assert_eq!(
        recall["inputSchema"]["properties"]["query"]["description"],
        "Words the notes should match. Omit to get every note for the repo."
    );
}

#[test]
fn recall_memory_labels_anchors_outside_their_repo_as_unchecked() {
    let home = memory_home();
    let note = "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H6F\ntype: fact\ntitle: Migrations keep a log\n\
anchors:\n  - path: src/db.rs\n    lines: 2-3\n    hash: b3:0123456789abcdef\n\
---\nEvery migration writes a log line.\n";
    add_note(home.path(), note, &[]);
    let elsewhere = home.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let reply = recall(
        home.path(),
        &home.path().join("vault"),
        json!({ "cwd": elsewhere.to_str().unwrap(), "query": "migration", "limit": 10 }),
    );
    assert_eq!(
        structured(&reply)["notes"],
        json!([{
            "id": "01JC9V6Q4M8K2T7X3N5R0B1H6F",
            "title": "Migrations keep a log",
            "scope": "global",
            "freshness": "unchecked",
            "excerpt": "Every migration writes a log line.",
        }])
    );
}

#[test]
fn recall_memory_without_a_vault_is_missing_and_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    let vault = home.path().join("none");
    let reply = recall(
        home.path(),
        &vault,
        json!({ "cwd": home.path().to_str().unwrap(), "query": "migration" }),
    );
    assert_eq!(
        structured(&reply),
        &json!({ "vault": "missing", "notes": [], "total": 0 })
    );
    assert!(!vault.exists(), "the server created a vault");
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn recall_memory_masks_a_secret_in_a_hand_written_note() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let note = format!(
        "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H6E\ntype: fact\ntitle: Deploy with ghp_{SECRET_BODY}\n\
scope: global\nstatus: active\ncreated: 2026-11-02\n---\n\
Deploy zanzibar with token ghp_{SECRET_BODY} only.\n"
    );
    std::fs::create_dir_all(vault.join("global")).unwrap();
    std::fs::write(vault.join("global/deploy.md"), note).unwrap();

    let reply = recall(
        home.path(),
        &vault,
        json!({ "cwd": home.path().to_str().unwrap(), "query": "zanzibar" }),
    );
    assert!(!reply.to_string().contains(SECRET_BODY), "{reply}");
    let note = &structured(&reply)["notes"][0];
    assert_eq!(note["title"], "Deploy with «redacted:github-token»");
    assert_eq!(
        note["excerpt"],
        "Deploy zanzibar with token «redacted:github-token» only."
    );
}

#[test]
fn recall_memory_drops_notes_from_the_tail_to_fit_the_reply() {
    let home = memory_home();
    let vault = home.path().join("vault");
    std::fs::create_dir_all(vault.join("global")).unwrap();
    for i in 0..10 {
        let note = format!(
            "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H{i:02}\ntype: fact\ntitle: Zanzibar {}\n\
scope: global\nstatus: active\ncreated: 2026-11-02\n---\nZanzibar body.\n",
            "w".repeat(600)
        );
        std::fs::write(vault.join(format!("global/n{i}.md")), note).unwrap();
    }
    let reply = recall(
        home.path(),
        &vault,
        json!({ "cwd": home.path().to_str().unwrap(), "query": "zanzibar", "limit": 10 }),
    );
    let content = structured(&reply);
    assert_eq!(content["truncated"], true);
    assert_eq!(content["notes"].as_array().unwrap().len(), 8);
    assert!(content.to_string().chars().count() <= 6_000);
}

#[test]
fn recall_memory_returns_ten_by_default_and_counts_every_note_in_scope() {
    let home = memory_home();
    let vault = home.path().join("vault");
    std::fs::create_dir_all(vault.join("global")).unwrap();
    for i in 0..12 {
        let note = format!(
            "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H{i:02}\ntype: fact\ntitle: Kestrel {i}\n\
scope: global\nstatus: active\ncreated: 2026-11-02\n---\nKestrel body.\n"
        );
        std::fs::write(vault.join(format!("global/k{i}.md")), note).unwrap();
    }
    let cwd = home.path().to_str().unwrap();
    for arguments in [
        json!({ "cwd": cwd }),
        json!({ "cwd": cwd, "query": "kestrel" }),
    ] {
        let reply = recall(home.path(), &vault, arguments.clone());
        let content = structured(&reply);
        assert_eq!(
            content["notes"].as_array().unwrap().len(),
            10,
            "{arguments}"
        );
        assert_eq!(content["total"], 12, "{arguments}");
        assert_eq!(content["truncated"], false, "{arguments}");
    }
    let reply = recall(home.path(), &vault, json!({ "cwd": cwd, "limit": 2 }));
    assert_eq!(structured(&reply)["notes"].as_array().unwrap().len(), 2);
    assert_eq!(structured(&reply)["total"], 12);
}

#[test]
fn recall_memory_rejects_bad_arguments_with_a_code_and_a_fix() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let cwd = home.path().to_str().unwrap();
    let cases = [
        (
            json!({ "cwd": cwd, "query": "x", "limit": 11 }),
            "limit",
            "must be an integer from 1 to 10",
        ),
        (
            json!({ "cwd": "src", "query": "x" }),
            "cwd",
            "must be an absolute path",
        ),
        (
            json!({ "cwd": cwd, "query": 5 }),
            "query",
            "must be a string",
        ),
    ];
    for (arguments, field, problem) in cases {
        let reply = recall(home.path(), &vault, arguments);
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        assert_eq!(
            reply["result"]["structuredContent"],
            json!({
                "code": "invalid_argument",
                "field": field,
                "message": format!("`{field}` {problem}"),
                "fix": format!("Fix `{field}`: {problem}, then call again."),
            })
        );
    }
}

const TIMELINE_NOTE_ID: &str = "01JC9V6Q4M8K2T7X3N5R0B1H7A";
const TIMELINE_NOTE: &str = "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H7A\ntype: pitfall\n\
title: Retry the migration\nscope: global\nstatus: active\ncreated: 2026-11-01\n\
anchors:\n  - path: src/db.rs\n    lines: 2-3\n    hash: b3:0123456789abcdef\n---\n\
Retry a failed migration once.\n\n## Timeline\n- 2026-11-01 written by hand\n\
- 2026-11-02 accepted from claude-code:s_1\n";

/// A memory home whose vault holds `text` at `rel`, written by hand.
fn vault_with(rel: &str, text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let home = memory_home();
    let vault = home.path().join("vault");
    let file = vault.join(rel);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
    (home, vault)
}

#[test]
fn read_note_by_id_drops_the_timeline_and_counts_its_entries() {
    let (home, vault) = vault_with("global/retry-the-migration.md", TIMELINE_NOTE);
    let reply = read_note(home.path(), &vault, json!({ "note": TIMELINE_NOTE_ID }));

    assert_eq!(
        structured(&reply),
        &json!({
            "id": TIMELINE_NOTE_ID,
            "title": "Retry the migration",
            "scope": "global",
            "status": "active",
            "freshness": "unchecked",
            "body": "Retry a failed migration once.",
            "timeline_entries": 2,
            "truncated": false,
        })
    );
}

#[test]
fn read_note_by_file_name_or_path() {
    let (home, vault) = vault_with("global/retry-the-migration.md", TIMELINE_NOTE);
    for key in [
        "retry-the-migration",
        "retry-the-migration.md",
        "global/retry-the-migration.md",
    ] {
        let reply = read_note(home.path(), &vault, json!({ "note": key }));
        assert_eq!(structured(&reply)["id"], TIMELINE_NOTE_ID, "{key}: {reply}");
    }
}

#[test]
fn read_note_rejects_paths_outside_the_vault() {
    let (home, vault) = vault_with("global/retry-the-migration.md", TIMELINE_NOTE);
    let problem = "must be a note id, file name, or path inside the vault";
    let cases = [
        (json!({ "note": "../etc/passwd" }), "note", problem),
        (json!({ "note": "/etc/passwd" }), "note", problem),
        (json!({ "note": "global/../../x.md" }), "note", problem),
        (json!({ "note": ".lore/x.md" }), "note", problem),
        (json!({}), "note", problem),
        (
            json!({ "note": TIMELINE_NOTE_ID, "cwd": "src" }),
            "cwd",
            "must be an absolute path",
        ),
    ];
    for (arguments, field, problem) in cases {
        let reply = read_note(home.path(), &vault, arguments);
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        assert_eq!(
            reply["result"]["structuredContent"],
            json!({
                "code": "invalid_argument",
                "field": field,
                "message": format!("`{field}` {problem}"),
                "fix": format!("Fix `{field}`: {problem}, then call again."),
            })
        );
    }
}

#[cfg(unix)]
#[test]
fn read_note_does_not_follow_a_symlink_out_of_the_vault() {
    let (home, vault) = vault_with("global/retry-the-migration.md", TIMELINE_NOTE);
    let outside = home.path().join("outside.md");
    std::fs::write(&outside, TIMELINE_NOTE.replace("1H7A", "1H7C")).unwrap();
    std::os::unix::fs::symlink(&outside, vault.join("global/escape.md")).unwrap();
    let reply = read_note(home.path(), &vault, json!({ "note": "global/escape.md" }));
    assert_eq!(
        reply["result"]["structuredContent"]["code"], "invalid_argument",
        "{reply}"
    );
}

#[test]
fn read_note_reports_unknown_and_ambiguous_notes() {
    let (home, vault) = vault_with("global/dup.md", TIMELINE_NOTE);
    let other = TIMELINE_NOTE.replace("1H7A", "1H7B");
    std::fs::create_dir_all(vault.join("repos/x")).unwrap();
    std::fs::write(vault.join("repos/x/dup.md"), other).unwrap();

    let reply = read_note(
        home.path(),
        &vault,
        json!({ "note": "01JC9V6Q4M8K2T7X3N5R0B1H7Z" }),
    );
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    assert_eq!(
        reply["result"]["structuredContent"],
        json!({
            "code": "note_not_found",
            "message": "no note matches `01JC9V6Q4M8K2T7X3N5R0B1H7Z`",
            "fix": "Call recall_memory to get a current note id.",
        })
    );

    let reply = read_note(home.path(), &vault, json!({ "note": "dup" }));
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    assert_eq!(
        reply["result"]["structuredContent"],
        json!({
            "code": "note_ambiguous",
            "message": "`dup` matches more than one note: global/dup.md, repos/x/dup.md",
            "fix": "Pass the note id instead of the slug.",
            "candidates": ["global/dup.md", "repos/x/dup.md"],
        })
    );
}

#[test]
fn read_note_without_a_vault_is_vault_missing_and_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    let vault = home.path().join("none");
    let reply = read_note(home.path(), &vault, json!({ "note": TIMELINE_NOTE_ID }));
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let dir = vault.display();
    assert_eq!(
        reply["result"]["structuredContent"],
        json!({
            "code": "vault_missing",
            "message": format!("no vault at {dir}"),
            "fix": format!(
                "No memory vault at {dir}. Continue without memory. \
        Tell the user `lorectl mem init` creates one."
            ),
        })
    );
    assert!(!vault.exists(), "the server created a vault");
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn read_note_masks_a_secret_in_a_hand_written_note() {
    let note = format!(
        "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H6E\ntype: fact\ntitle: Deploy with ghp_{SECRET_BODY}\n\
scope: global\nstatus: active\ncreated: 2026-11-02\n---\n\
Deploy zanzibar with token ghp_{SECRET_BODY} only.\n"
    );
    let (home, vault) = vault_with("global/deploy.md", &note);
    let reply = read_note(home.path(), &vault, json!({ "note": "deploy" }));
    assert!(!reply.to_string().contains(SECRET_BODY), "{reply}");
    let read = structured(&reply);
    assert_eq!(read["title"], "Deploy with «redacted:github-token»");
    assert_eq!(
        read["body"],
        "Deploy zanzibar with token «redacted:github-token» only."
    );
    assert_eq!(read["freshness"], "unanchored");
    assert_eq!(read["timeline_entries"], 0);
}

#[test]
fn read_note_cuts_a_long_body() {
    let note = TIMELINE_NOTE.replace("Retry a failed", &"w".repeat(9_000));
    let (home, vault) = vault_with("global/long.md", &note);
    let reply = read_note(home.path(), &vault, json!({ "note": "long" }));
    let read = structured(&reply);
    assert_eq!(read["truncated"], true);
    assert_eq!(read["body"], "w".repeat(8_000));
}

#[test]
fn read_note_checks_freshness_against_the_cwd_repo() {
    let home = memory_home();
    let repo = home.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:hsusul/lore.git"],
    );
    std::fs::write(repo.join("src/a.rs"), "fn retry() {}\n").unwrap();
    let cwd = repo.to_str().unwrap();
    add_note(home.path(), REPO_NOTE, &["--cwd", cwd]);
    let vault = home.path().join("vault");
    let elsewhere = home.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let id = "01JC9V6Q4M8K2T7X3N5R0B1H6D";
    let cases = [
        (json!({ "note": id, "cwd": cwd }), "fresh"),
        (json!({ "note": id }), "unchecked"),
        (
            json!({ "note": id, "cwd": elsewhere.to_str().unwrap() }),
            "unchecked",
        ),
    ];
    for (arguments, freshness) in cases {
        let reply = read_note(home.path(), &vault, arguments);
        assert_eq!(structured(&reply)["freshness"], freshness, "{reply}");
    }
}

/// `initialize` as `client` (no `clientInfo` when `None`), then `calls`, on
/// one server with `HOME` at `home`, the vault at `vault`, and `extra` env.
/// Returns the replies to `calls`.
fn session(
    home: &Path,
    vault: &Path,
    extra: &[(&str, &str)],
    client: Option<&str>,
    calls: &[Value],
) -> Vec<Value> {
    let mut envs = vec![
        GIT_ENV[0],
        GIT_ENV[1],
        ("HOME", home.to_str().unwrap()),
        ("LORE_VAULT_DIR", vault.to_str().unwrap()),
    ];
    envs.extend_from_slice(extra);
    let mut params = json!({ "protocolVersion": "2025-06-18" });
    if let Some(name) = client {
        params["clientInfo"] = json!({ "name": name, "version": "1.0.0" });
    }
    let messages = [vec![request(0, "initialize", params)], calls.to_vec()].concat();
    let mut replies = converse_with(&home.join("archive"), &envs, &messages);
    assert_eq!(replies.remove(0)["id"], 0);
    for reply in &replies {
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap(),
            reply["result"]["structuredContent"],
            "the text content carries the same JSON"
        );
    }
    replies
}

fn propose(id: i64, arguments: Value) -> Value {
    call(id, "propose_memory", arguments)
}

/// A git repository at `<home>/repo` whose key is `81444c90f1ff44ac`, with
/// `src/a.rs` holding one line.
fn lore_repo(home: &Path) -> String {
    let repo = home.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:hsusul/lore.git"],
    );
    std::fs::write(repo.join("src/a.rs"), "fn retry() {}\n").unwrap();
    repo.to_str().unwrap().to_owned()
}

/// Sorted file names in `<vault>/_inbox`, none when it does not exist.
fn inbox_files(vault: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(vault.join("_inbox"))
        .map(|dir| {
            dir.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn git_status(vault: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(["status", "--porcelain"])
        .envs(GIT_ENV)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn propose_memory_files_into_the_connection_inbox_as_claude_code() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let cwd = lore_repo(home.path());
    let arguments = json!({
        "title": "Retry the migration once",
        "body": "A failed migration succeeds on the second try.\n",
        "type": "pitfall",
        "scope": "repo",
        "cwd": cwd,
        "anchors": [{ "path": "src/a.rs", "lines": "1" }],
    });
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("claude-code"),
        &[propose(1, arguments)],
    );

    let reply = structured(&replies[0]);
    let session_id = reply["session"].as_str().unwrap();
    // `mcp-<ULID>` with a `.` between the ULID's time and random parts.
    let (time, random) = session_id
        .strip_prefix("mcp-")
        .and_then(|ulid| ulid.split_once('.'))
        .unwrap();
    let ulid = format!("{time}{random}");
    assert_eq!((time.len(), ulid.len()), (10, 26), "{session_id}");
    assert!(ulid
        .bytes()
        .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase()));
    let id = reply["proposal_id"].as_str().unwrap();
    let inbox = format!(
        "_inbox/claude-code-{}.jsonl",
        session_id.to_ascii_lowercase()
    );
    assert_eq!(
        reply,
        &json!({
            "proposal_id": id,
            "inbox": inbox,
            "session": session_id,
            "source_verified": false,
        })
    );

    let text = std::fs::read_to_string(vault.join(&inbox)).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    let mut line: Value = serde_json::from_str(text.trim_end()).unwrap();
    let mut listed: Value = serde_json::from_str(&mem(home.path(), &["inbox", "--json"])).unwrap();
    let receipt = listed[0].as_object_mut().unwrap().remove("receipt");
    assert_eq!(receipt, Some(json!({ "status": "no_archive" })));
    assert_eq!(listed, json!([line.clone()]));
    assert!(line["proposed_at"].take().is_string());
    assert_eq!(
        line,
        json!({
            "id": id,
            "agent": "claude-code",
            "session": session_id,
            "source_verified": false,
            "type": "pitfall",
            "scope": "repo:81444c90f1ff44ac",
            "title": "Retry the migration once",
            "body": "A failed migration succeeds on the second try.\n",
            "anchors": [{ "path": "src/a.rs", "lines": "1", "hash": "b3:72208a5e4963e1b2" }],
            "proposed_at": null,
        })
    );
    assert_eq!(git_status(&vault), "", "the vault's git status is clean");
}

#[test]
fn propose_memory_takes_the_agent_from_lore_agent_then_client_info() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let note =
        json!({ "title": "Keep the log short", "body": "B", "type": "fact", "scope": "global" });
    let cases = [
        (Some("codex"), Some("claude-code"), "codex"),
        (Some("claude-code"), Some("codex-tui"), "claude-code"),
        (None, Some("codex-mcp-client"), "codex"),
    ];
    for (env, client, agent) in cases {
        let extra: Vec<(&str, &str)> = env.map(|e| ("LORE_AGENT", e)).into_iter().collect();
        let replies = session(
            home.path(),
            &vault,
            &extra,
            client,
            &[propose(1, note.clone())],
        );
        let inbox = structured(&replies[0])["inbox"].as_str().unwrap();
        assert!(
            inbox.starts_with(&format!("_inbox/{agent}-mcp-")),
            "{env:?} {client:?}: {inbox}"
        );
    }
    assert_eq!(inbox_files(&vault).len(), 3);
    assert_eq!(git_status(&vault), "");
}

#[test]
fn propose_memory_from_an_unknown_agent_writes_nothing() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let note =
        json!({ "title": "Keep the log short", "body": "B", "type": "fact", "scope": "global" });
    for client in [None, Some("cursor")] {
        let replies = session(
            home.path(),
            &vault,
            &[],
            client,
            &[propose(1, note.clone())],
        );
        assert_eq!(replies[0]["result"]["isError"], true, "{client:?}");
        assert_eq!(
            replies[0]["result"]["structuredContent"],
            json!({
                "code": "agent_unknown",
                "message": "cannot tell which agent is calling",
                "fix": "Ask the user to set LORE_AGENT=claude-code or codex in the MCP server config.",
            })
        );
    }
    assert_eq!(inbox_files(&vault), Vec::<String>::new());
}

#[test]
fn propose_memory_stores_a_secret_masked() {
    let home = memory_home();
    let vault = home.path().join("vault");
    // `AKIAIOSFODNN7EXAMPLE` is on the scanner's allowlist, so use another.
    let key = format!("{}{}", "AKIA", "ABCDEFGHIJKLMNOP");
    let note = json!({
        "title": "Rotate the deploy key",
        "body": format!("Deploy with {key} only."),
        "type": "fact",
        "scope": "global",
        "session_id": "0199-native-session",
    });
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("claude-code"),
        &[propose(1, note)],
    );
    let reply = structured(&replies[0]);
    assert_eq!(reply["session"], "0199-native-session");
    assert_eq!(reply["source_verified"], false);
    assert_eq!(
        reply["inbox"],
        "_inbox/claude-code-0199-native-session.jsonl"
    );
    let text = std::fs::read_to_string(vault.join("_inbox/claude-code-0199-native-session.jsonl"))
        .unwrap();
    assert!(!text.contains(&key), "{text}");
    let line: Value = serde_json::from_str(text.trim_end()).unwrap();
    assert_eq!(
        line["body"],
        "Deploy with «redacted:aws-access-key-id» only."
    );
    assert_eq!(git_status(&vault), "");
}

#[test]
fn propose_memory_rejects_bad_arguments_with_a_code_and_a_fix() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let cwd = lore_repo(home.path());
    let ok = json!({ "title": "Retry the migration once", "body": "B", "type": "fact", "scope": "repo", "cwd": cwd });
    let with = |key: &str, value: Value| {
        let mut arguments = ok.clone();
        arguments[key] = value;
        arguments
    };
    let nine: Vec<Value> = (0..9).map(|_| json!({ "path": "src/a.rs" })).collect();
    let cases = [
        (
            with("body", json!("Do it.\n\n## Timeline\n- 2026-11-02 x\n")),
            "body",
            "the `## Timeline` section is Lore's to write",
        ),
        (
            with("title", json!("Retry the\nmigration once")),
            "title",
            "must be one line without control characters",
        ),
        (
            with("title", json!("Use pnpm")),
            "title",
            "must be a whole rule of at least three words",
        ),
        (
            with("title", json!("Cause you never get to access it")),
            "title",
            "must make sense without the conversation: say what it is about instead of starting with `Cause`",
        ),
        (
            with("title", json!("t".repeat(121))),
            "title",
            "must be at most 120 characters",
        ),
        (
            with("body", json!("b".repeat(4_001))),
            "body",
            "must be at most 4000 characters",
        ),
        (
            with("anchors", json!(nine)),
            "anchors",
            "must be at most 8 anchors",
        ),
        (
            with("anchors", json!([{ "path": "/etc/passwd" }])),
            "anchors[0].path",
            "absolute paths are not allowed, got `/etc/passwd`",
        ),
        (
            with("scope", json!("everywhere")),
            "scope",
            "must be global or repo",
        ),
        (
            with("type", json!("rumor")),
            "type",
            "must be one of convention, decision, fact, howto, pitfall",
        ),
        (with("cwd", json!("src")), "cwd", "must be an absolute path"),
        (
            with("session_id", json!("a b")),
            "session_id",
            "must be letters, digits, `.`, `_`, `-`",
        ),
        (
            with("updates", json!("nope")),
            "updates",
            "must be a note id",
        ),
        (
            with("enforce", json!([{ "tool": "bash", "pattern": "(" }])),
            "enforce[0].pattern",
            "`(` is not a valid regex: error: unclosed group",
        ),
        (
            with("enforce", json!([{ "tool": "bash", "pattern": ".*" }])),
            "enforce[0].pattern",
            "`.*` matches empty text, so it would deny every call",
        ),
        (
            with("enforce", json!([{ "tool": "Bash", "pattern": "x" }])),
            "enforce[0].tool",
            "expected bash, write, or path, got `Bash`",
        ),
        (
            with("enforce", json!("bash")),
            "enforce",
            "must be a list",
        ),
    ];
    for (arguments, field, problem) in cases {
        let replies = session(
            home.path(),
            &vault,
            &[],
            Some("claude-code"),
            &[propose(1, arguments)],
        );
        assert_eq!(replies[0]["result"]["isError"], true, "{field}");
        assert_eq!(
            replies[0]["result"]["structuredContent"],
            json!({
                "code": "invalid_argument",
                "field": field,
                "message": format!("`{field}` {problem}"),
                "fix": format!("Fix `{field}`: {problem}, then call again."),
            })
        );
    }
    assert_eq!(inbox_files(&vault), Vec::<String>::new());
}

#[test]
fn propose_memory_with_enforce_round_trips_through_accept_into_the_note() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let arguments = json!({
        "title": "Never add an AI co-author line",
        "body": "Commit messages carry no AI co-author trailer.\n",
        "type": "convention",
        "scope": "global",
        "enforce": [{ "tool": "bash", "pattern": r"(?i)co-authored-by:\s*claude", "unless": "it's fine" }],
    });
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("claude-code"),
        &[propose(1, arguments)],
    );
    let id = structured(&replies[0])["proposal_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // The person sees the rule before accepting it.
    let listed = mem(home.path(), &["inbox"]);
    assert!(
        listed.contains(r"  enforce bash '(?i)co-authored-by:\s*claude' unless 'it's fine'"),
        "{listed}"
    );
    let accepted = mem(home.path(), &["accept", &id]);
    let rel = accepted.split_whitespace().nth(1).unwrap();
    let text = std::fs::read_to_string(vault.join(rel)).unwrap();
    assert!(
        text.contains(
            "enforce:\n  - tool: bash\n    pattern: '(?i)co-authored-by:\\s*claude'\n    unless: 'it''s fine'\n---\n"
        ),
        "{text}"
    );

    // An update can add a rule to a note that had none.
    let plain = json!({
        "title": "Never commit planning docs",
        "body": "Planning docs stay out of commits.\n",
        "type": "convention",
        "scope": "global",
    });
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("claude-code"),
        &[propose(1, plain)],
    );
    let first = structured(&replies[0])["proposal_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let accepted = mem(home.path(), &["accept", &first]);
    let (rel, note_id) = {
        let mut words = accepted.split_whitespace().skip(1);
        (
            words.next().unwrap().to_owned(),
            words.next().unwrap().to_owned(),
        )
    };
    let update = json!({
        "title": "Never commit planning docs",
        "body": "Planning docs stay out of commits; write them outside the repo.\n",
        "type": "convention",
        "scope": "global",
        "updates": note_id,
        "enforce": [{ "tool": "path", "pattern": r"(^|/)PLAN\.md$" }],
    });
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("claude-code"),
        &[propose(1, update)],
    );
    let second = structured(&replies[0])["proposal_id"]
        .as_str()
        .unwrap()
        .to_owned();
    mem(home.path(), &["accept", &second]);
    let text = std::fs::read_to_string(vault.join(&rel)).unwrap();
    assert!(
        text.contains("enforce:\n  - tool: path\n    pattern: '(^|/)PLAN\\.md$'\n"),
        "{text}"
    );
}

#[test]
fn propose_memory_pins_anchors_only_inside_the_scope_repo() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let cwd = lore_repo(home.path());
    let elsewhere = home.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let anchor_fix = "Anchor paths are relative to the repo root and must exist. \
                      Check path and lines, or omit lines.";
    let scope_fix = "Anchors need scope repo and a cwd inside that repo.";
    let note = |scope: &str, cwd: &str, anchor: Value| {
        json!({ "title": "Keep the log short", "body": "B", "type": "fact", "scope": scope, "cwd": cwd,
                "anchors": [anchor] })
    };
    let cases = [
        (
            note("repo", &cwd, json!({ "path": "src/missing.rs" })),
            "anchor_not_found",
            "anchor `src/missing.rs` is not a file in the repository".to_owned(),
            anchor_fix,
        ),
        (
            note("repo", &cwd, json!({ "path": "src/a.rs", "lines": "5-9" })),
            "anchor_not_found",
            "anchor `src/a.rs` has 1 lines, so lines 5-9 do not exist".to_owned(),
            anchor_fix,
        ),
        (
            note("global", &cwd, json!({ "path": "src/a.rs" })),
            "scope_mismatch",
            "anchors need scope repo".to_owned(),
            scope_fix,
        ),
        (
            note(
                "repo",
                elsewhere.to_str().unwrap(),
                json!({ "path": "src/a.rs" }),
            ),
            "scope_mismatch",
            format!(
                "{} is not inside a git repository with an origin remote",
                elsewhere.display()
            ),
            scope_fix,
        ),
    ];
    for (arguments, code, message, fix) in cases {
        let replies = session(
            home.path(),
            &vault,
            &[],
            Some("claude-code"),
            &[propose(1, arguments)],
        );
        assert_eq!(replies[0]["result"]["isError"], true, "{code}");
        assert_eq!(
            replies[0]["result"]["structuredContent"],
            json!({ "code": code, "message": message, "fix": fix })
        );
    }
    assert_eq!(inbox_files(&vault), Vec::<String>::new());
}

#[test]
fn propose_memory_updates_only_a_note_that_exists() {
    let home = memory_home();
    let vault = home.path().join("vault");
    add_note(home.path(), TIMELINE_NOTE, &[]);
    let note = |target: &str| {
        json!({ "title": "Keep the log short", "body": "B", "type": "pitfall", "scope": "global",
                "updates": target })
    };
    let unknown = "01JC9V6Q4M8K2T7X3N5R0B1H7Z";
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("claude-code"),
        &[
            propose(1, note(unknown)),
            propose(2, note(TIMELINE_NOTE_ID)),
        ],
    );
    assert_eq!(replies[0]["result"]["isError"], true);
    assert_eq!(
        replies[0]["result"]["structuredContent"],
        json!({
            "code": "note_not_found",
            "message": format!("no note matches `{unknown}`"),
            "fix": "Call recall_memory to get a current note id.",
        })
    );
    let inbox = structured(&replies[1])["inbox"].as_str().unwrap();
    let text = std::fs::read_to_string(vault.join(inbox)).unwrap();
    assert_eq!(text.lines().count(), 1, "only the update was written");
    let line: Value = serde_json::from_str(text.trim_end()).unwrap();
    assert_eq!(line["relation"], json!({ "updates": TIMELINE_NOTE_ID }));
    assert_eq!(git_status(&vault), "");
}

#[test]
fn propose_memory_without_a_vault_is_vault_missing_and_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    let vault = home.path().join("none");
    let note =
        json!({ "title": "Keep the log short", "body": "B", "type": "fact", "scope": "global" });
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("claude-code"),
        &[propose(1, note)],
    );
    let dir = vault.display();
    assert_eq!(
        replies[0]["result"]["structuredContent"],
        json!({
            "code": "vault_missing",
            "message": format!("no vault at {dir}"),
            "fix": format!(
                "No memory vault at {dir}. Continue without memory. \
        Tell the user `lorectl mem init` creates one."
            ),
        })
    );
    assert!(!vault.exists(), "the server created a vault");
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn two_servers_proposing_at_once_write_two_whole_files() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let calls = |server: usize| -> Vec<Value> {
        (0..25)
            .map(|i| {
                let body = format!("Server {server} proposal {i}. {}", "x".repeat(3_000));
                propose(
                    i,
                    json!({ "title": "Keep the log short", "body": body, "type": "fact", "scope": "global" }),
                )
            })
            .collect()
    };
    let sessions: Vec<String> = std::thread::scope(|scope| {
        let servers: Vec<_> = (0..2)
            .map(|server| {
                let (home, vault, calls) = (home.path(), &vault, calls(server));
                scope.spawn(move || session(home, vault, &[], Some("claude-code"), &calls))
            })
            .collect();
        servers
            .into_iter()
            .map(|s| {
                let replies = s.join().unwrap();
                assert_eq!(replies.len(), 25);
                structured(&replies[0])["session"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    });
    assert_ne!(sessions[0], sessions[1]);
    let files = inbox_files(&vault);
    assert_eq!(files.len(), 2, "{files:?}");
    for name in files {
        let text = std::fs::read_to_string(vault.join("_inbox").join(&name)).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).expect("a whole line"))
            .collect();
        assert_eq!(lines.len(), 25, "{name}");
        let server = &lines[0]["body"].as_str().unwrap()[..8];
        assert!(lines
            .iter()
            .all(|l| l["body"].as_str().unwrap().starts_with(server)));
    }
    assert_eq!(git_status(&vault), "");
}

/// Scan the one Claude Code session from [`plant_session`] into
/// `<home>/archive`, where [`session`] points the server. Returns its archive id.
/// Reading the id opens the archive read-only, which leaves the WAL sidecars a
/// server read would, so a later [`snapshot`] compares every byte after that.
fn archive_in(home: &Path) -> String {
    let homes = common::empty_homes();
    plant_session(homes.path(), 1);
    let archive = home.join("archive");
    let out = lorectl(&archive, homes.path(), &["scan"]);
    assert_eq!(out.status.code(), Some(0), "seeding scan failed");
    only_session_id(&archive)
}

/// Every file under `dir`, with its bytes, sorted by path.
fn snapshot(dir: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                files.push((path, bytes));
            }
        }
    }
    files.sort();
    files
}

fn global_note(session_id: &str) -> Value {
    json!({
        "title": "Keep the log short", "body": "B", "type": "fact", "scope": "global",
        "session_id": session_id,
    })
}

#[test]
fn propose_memory_verifies_a_session_of_the_calling_agent() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let archive_id = archive_in(home.path());
    let before = snapshot(&home.path().join("archive"));
    assert!(archive_id.starts_with("s_"), "{archive_id}");
    for given in [SESSION_UUID, archive_id.as_str()] {
        let replies = session(
            home.path(),
            &vault,
            &[],
            Some("claude-code"),
            &[propose(1, global_note(given))],
        );
        let reply = structured(&replies[0]);
        let id = reply["proposal_id"].as_str().unwrap();
        assert_eq!(
            reply,
            &json!({
                "proposal_id": id,
                "inbox": format!("_inbox/claude-code-{archive_id}.jsonl"),
                "session": archive_id,
                "source_verified": true,
            }),
            "{given}"
        );
    }
    let file = format!("claude-code-{archive_id}.jsonl");
    assert_eq!(inbox_files(&vault), vec![file.clone()]);
    let text = std::fs::read_to_string(vault.join("_inbox").join(file)).unwrap();
    let sessions: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["session"].clone())
        .collect();
    assert_eq!(sessions, vec![json!(archive_id), json!(archive_id)]);
    let verified: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["source_verified"].clone())
        .collect();
    assert_eq!(verified, vec![json!(true), json!(true)]);
    let listed = mem(home.path(), &["inbox"]);
    assert_eq!(
        listed
            .matches(&format!("claude-code:{archive_id} (verified)"))
            .count(),
        2,
        "{listed}"
    );
    assert_eq!(
        snapshot(&home.path().join("archive")),
        before,
        "archive unchanged"
    );
}

#[test]
fn propose_memory_refuses_a_session_of_another_agent_and_writes_nothing() {
    let home = memory_home();
    let vault = home.path().join("vault");
    archive_in(home.path());
    let before = snapshot(&home.path().join("archive"));
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("codex"),
        &[propose(1, global_note(SESSION_UUID))],
    );
    assert_eq!(replies[0]["result"]["isError"], true);
    assert_eq!(
        replies[0]["result"]["structuredContent"],
        json!({
            "code": "session_mismatch",
            "message": format!("session `{SESSION_UUID}` was recorded by claude-code"),
            "fix": "That session belongs to another agent. Omit session_id.",
        })
    );
    assert_eq!(inbox_files(&vault), Vec::<String>::new());
    assert_eq!(git_status(&vault), "");
    assert_eq!(
        snapshot(&home.path().join("archive")),
        before,
        "archive unchanged"
    );
}

#[test]
fn propose_memory_stores_an_unknown_session_as_given_and_unverified() {
    let home = memory_home();
    let vault = home.path().join("vault");
    archive_in(home.path());
    let before = snapshot(&home.path().join("archive"));
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("claude-code"),
        &[propose(1, global_note("Not-Archived-1"))],
    );
    let reply = structured(&replies[0]);
    assert_eq!(
        reply,
        &json!({
            "proposal_id": reply["proposal_id"].as_str().unwrap(),
            "inbox": "_inbox/claude-code-not-archived-1.jsonl",
            "session": "Not-Archived-1",
            "source_verified": false,
        })
    );
    assert_eq!(
        inbox_files(&vault),
        vec!["claude-code-not-archived-1.jsonl"]
    );
    assert_eq!(
        snapshot(&home.path().join("archive")),
        before,
        "archive unchanged"
    );
}

#[test]
fn an_archive_shaped_session_the_archive_lacks_is_stored_unverified() {
    let home = memory_home();
    let vault = home.path().join("vault");
    archive_in(home.path());
    let fake = "s_0123456789abcdef";
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("claude-code"),
        &[propose(1, global_note(fake))],
    );
    let id = structured(&replies[0])["proposal_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let text =
        std::fs::read_to_string(vault.join(format!("_inbox/claude-code-{fake}.jsonl"))).unwrap();
    let line: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(line["session"], fake);
    assert_eq!(line["source_verified"], false);
    assert_eq!(
        mem(home.path(), &["inbox"]),
        format!(
            "{id}  claude-code:{fake} (unverified)  fact  global  Keep the log short\n\
             \x20   receipt: no archive yet; `lorectl scan` builds one\n"
        )
    );
}

#[test]
fn propose_memory_without_an_archive_stores_the_session_unverified() {
    let home = memory_home();
    let vault = home.path().join("vault");
    let replies = session(
        home.path(),
        &vault,
        &[],
        Some("claude-code"),
        &[propose(1, global_note(SESSION_UUID))],
    );
    let reply = structured(&replies[0]);
    assert_eq!(
        reply,
        &json!({
            "proposal_id": reply["proposal_id"].as_str().unwrap(),
            "inbox": format!("_inbox/claude-code-{SESSION_UUID}.jsonl"),
            "session": SESSION_UUID,
            "source_verified": false,
        })
    );
    assert_eq!(
        inbox_files(&vault),
        vec![format!("claude-code-{SESSION_UUID}.jsonl")]
    );
    assert!(
        !home.path().join("archive").exists(),
        "the server created an archive"
    );
}

#[test]
fn search_lists_a_session_once_with_its_match_count() {
    // Every assistant reply in this session says "step", so it used to fill
    // the page with copies of itself.
    let (archive, _homes) = scanned_archive(5);
    let replies = converse(
        archive.path(),
        &[call(1, "search_sessions", json!({ "query": "step" }))],
    );
    let hits = structured(&replies[0])["hits"].as_array().unwrap();
    let found: Vec<(&str, u64)> = hits
        .iter()
        .map(|h| {
            (
                h["session_id"].as_str().unwrap(),
                h["matches"].as_u64().unwrap(),
            )
        })
        .collect();
    let session_id = only_session_id(archive.path());
    assert_eq!(found, [(session_id.as_str(), 5)]);
}

#[test]
fn read_session_returns_all_of_a_document_longer_than_a_page() {
    let archive = tempfile::tempdir().unwrap();
    let homes = common::empty_homes();
    let project = homes.path().join("claude/projects/p");
    std::fs::create_dir_all(&project).unwrap();
    let long: String = (0..5_000).map(|i| format!("word{i:05} ")).collect();
    std::fs::write(
        project.join(format!("{SESSION_UUID}.jsonl")),
        format!(
            "{{\"type\":\"user\",\"uuid\":\"u0\",\"sessionId\":\"{SESSION_UUID}\",\"cwd\":\"/p\",\
\"message\":{{\"role\":\"user\",\"content\":\"summarise\"}}}}\n\
{{\"type\":\"assistant\",\"uuid\":\"a0\",\"parentUuid\":\"u0\",\"sessionId\":\"{SESSION_UUID}\",\
\"cwd\":\"/p\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{long}\"}}]}}}}\n"
        ),
    )
    .unwrap();
    let out = lorectl(archive.path(), homes.path(), &["scan"]);
    assert_eq!(out.status.code(), Some(0), "seeding scan failed");
    let session_id = only_session_id(archive.path());

    let mut assistant = String::new();
    let mut cursor: Option<String> = None;
    for _ in 0..10 {
        let mut args = json!({ "session_id": session_id });
        if let Some(cursor) = &cursor {
            args["cursor"] = json!(cursor);
        }
        let replies = converse(archive.path(), &[call(1, "read_session", args)]);
        let page = structured(&replies[0]);
        for entry in page["entries"].as_array().unwrap() {
            if entry["label"] == "assistant" {
                assistant.push_str(entry["text"].as_str().unwrap());
            }
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        (assistant.chars().count(), assistant.trim_end()),
        (long.chars().count(), long.trim_end()),
        "read_session lost part of a long reply"
    );
}
