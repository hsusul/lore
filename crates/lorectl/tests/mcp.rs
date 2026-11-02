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
            "read_note"
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
fn mem(home: &Path, args: &[&str]) {
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
        &json!({ "vault": "ok", "notes": [], "truncated": false })
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
        &json!({ "vault": "missing", "notes": [] })
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
            json!({ "cwd": cwd, "query": "--" }),
            "query",
            "must contain at least one word",
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
