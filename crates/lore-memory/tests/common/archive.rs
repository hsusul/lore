//! Scratch archives of Codex sessions for the miner tests.
#![allow(clippy::expect_used, clippy::unwrap_used, dead_code)]

use std::path::Path;

use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
use lore_core::discovery::{discover, DiscoveryConfig};
use lore_core::ingest::ingest_discovered;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

/// One Codex rollout per session, each a list of `(role, text)` turns, in
/// `dir`, ingested into an in-memory archive. Session `i` starts on
/// 2026-08-`i+1` and its turns are a second apart.
pub fn codex_archive(dir: &Path, sessions: &[&[(&str, &str)]]) -> Connection {
    let root = dir.join("codex/sessions");
    for (i, turns) in sessions.iter().enumerate() {
        let day = root.join(format!("2026/08/{:02}", i + 1));
        std::fs::create_dir_all(&day).unwrap();
        let ts = |s: usize| format!("2026-08-{:02}T10:00:{:02}.000Z", i + 1, s);
        let mut lines = vec![serde_json::json!({
            "type": "session_meta", "timestamp": ts(0),
            "payload": {"id": format!("019e0000-0000-7000-8000-{:012x}", i + 1),
                "cwd": "/proj", "cli_version": "0.133.0", "model_provider": "openai"}
        })];
        for (n, (role, text)) in turns.iter().enumerate() {
            let kind = if *role == "user" {
                "input_text"
            } else {
                "output_text"
            };
            lines.push(serde_json::json!({
                "type": "response_item", "timestamp": ts(n + 1),
                "payload": {"type": "message", "role": role,
                    "content": [{"type": kind, "text": text}]}
            }));
        }
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(day.join(format!("rollout-{:012x}.jsonl", i + 1)), body).unwrap();
    }
    let mut config = DiscoveryConfig::new();
    config.set_roots("codex", DiscoveryRoots::new(vec![root]));
    config.set_roots("claude-code", DiscoveryRoots::new(vec![dir.join("none")]));
    let registry = AdapterRegistry::v0();
    let found = discover(&registry, &config);
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blobs = BlobStore::open(dir.join("blobs")).unwrap();
    let report = ingest_discovered(&conn, &registry, &found.sessions, &blobs);
    assert!(report.failures.is_empty(), "{:?}", report.failures.len());
    conn
}

/// The archive id of the session started on 2026-08-`n`.
pub fn session_id(conn: &Connection, n: usize) -> String {
    conn.query_row(
        "SELECT id FROM agent_session WHERE native_session_id = ?1",
        [format!("019e0000-0000-7000-8000-{n:012x}")],
        |r| r.get(0),
    )
    .unwrap()
}

/// One Claude Code transcript per `(session uuid, turns)`, each turn a
/// `(role, text, timestamp)`, in `dir`, ingested into an in-memory archive.
/// Resume and fork copies are two uuids with the same turns and timestamps.
/// A Claude Code turn: role, text, and RFC 3339 timestamp.
pub type Turn<'a> = (&'a str, &'a str, &'a str);

pub fn claude_archive(dir: &Path, sessions: &[(&str, &[Turn<'_>])]) -> Connection {
    let project = dir.join("claude/projects/-repo-app");
    std::fs::create_dir_all(&project).unwrap();
    for (sid, turns) in sessions {
        let mut prev: Option<String> = None;
        let mut lines = Vec::new();
        for (n, (role, text, ts)) in turns.iter().enumerate() {
            let uuid = format!("{}-{n:04}", &sid[..30]);
            let message = if *role == "user" {
                serde_json::json!({"role": "user", "content": text})
            } else if *role == "skill" {
                // An assistant turn that only loads a skill, as Claude Code logs one.
                serde_json::json!({"role": "assistant", "model": "claude-x",
                    "content": [{"type": "tool_use", "id": format!("toolu_{n:04}"),
                        "name": "Skill", "input": {"skill": text}}]})
            } else {
                serde_json::json!({"role": "assistant", "model": "claude-x",
                    "content": [{"type": "text", "text": text}]})
            };
            lines.push(serde_json::json!({
                "type": if *role == "skill" { "assistant" } else { role }, "uuid": uuid, "parentUuid": prev, "sessionId": sid,
                "timestamp": ts, "cwd": "/repo/app", "version": "2.1.0",
                "isSidechain": false, "userType": "external", "message": message,
            }));
            prev = Some(uuid);
        }
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(project.join(format!("{sid}.jsonl")), body).unwrap();
    }
    ingest(dir, "claude-code", dir.join("claude/projects"))
}

/// The archive id of the Claude Code session `uuid`.
pub fn claude_session_id(conn: &Connection, uuid: &str) -> String {
    conn.query_row(
        "SELECT id FROM agent_session WHERE native_session_id = ?1",
        [uuid],
        |r| r.get(0),
    )
    .unwrap()
}

fn ingest(dir: &Path, agent: &str, root: std::path::PathBuf) -> Connection {
    let mut config = DiscoveryConfig::new();
    for name in ["codex", "claude-code"] {
        let roots = if name == agent {
            vec![root.clone()]
        } else {
            vec![dir.join("none")]
        };
        config.set_roots(name, DiscoveryRoots::new(roots));
    }
    let registry = AdapterRegistry::v0();
    let found = discover(&registry, &config);
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blobs = BlobStore::open(dir.join("blobs")).unwrap();
    let report = ingest_discovered(&conn, &registry, &found.sessions, &blobs);
    assert!(report.failures.is_empty(), "{:?}", report.failures.len());
    conn
}
