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
    let mut child = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("mcp")
        .arg("--archive")
        .arg(archive)
        .env_remove("LORE_ARCHIVE_DIR")
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
    assert_eq!(names, ["search_sessions", "read_session"]);
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
