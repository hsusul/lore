//! A source parsed into a different session leaves its old one. When the old
//! session has no source left it is deleted, and the folder the user filed it
//! in must follow the transcript that left. A session that keeps another
//! source keeps its folder, and the transcript that left it takes none.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::{
    AgentAdapter, AgentId, AgentMetadata, Capabilities, Detection, DiscoveryRoots, SessionRef,
};
use lore_core::ingest::{ingest_file, IngestOutcome};
use lore_core::model::ParsedSession;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

const PARENT: &str = "dddddddd-0000-4000-8000-000000000001";
const AGENT: &str = "a0000000000000002";

/// The Claude adapter with the session a file is keyed by fixed here. `fold`
/// keys a file by the first `sessionId` it carries, so a subagent transcript
/// takes its parent's session. Otherwise a line that carries `agentId` keys
/// the file `<sessionId>/agent-<agentId>`, which moves the subagent out.
struct Keyed {
    inner: ClaudeCodeAdapter,
    fold: bool,
}

impl Keyed {
    fn fold() -> Self {
        Self {
            inner: ClaudeCodeAdapter::new(),
            fold: true,
        }
    }

    fn split() -> Self {
        Self {
            inner: ClaudeCodeAdapter::new(),
            fold: false,
        }
    }
}

impl AgentAdapter for Keyed {
    fn id(&self) -> AgentId {
        self.inner.id()
    }
    fn metadata(&self) -> AgentMetadata {
        self.inner.metadata()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    fn detect_installation(&self, roots: &DiscoveryRoots) -> Detection {
        self.inner.detect_installation(roots)
    }
    fn roots(&self, overrides: &DiscoveryRoots) -> Vec<PathBuf> {
        self.inner.roots(overrides)
    }
    fn discover_sessions(&self, roots: &DiscoveryRoots) -> Vec<SessionRef> {
        self.inner.discover_sessions(roots)
    }
    fn parse_content(&self, content: &str, fallback_dedupe: &str) -> ParsedSession {
        let mut parsed = self.inner.parse_content(content, fallback_dedupe);
        parsed.native_session_id = content.lines().find_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            let session = v.get("sessionId")?.as_str()?;
            match v.get("agentId").and_then(|a| a.as_str()) {
                Some(agent) if !self.fold => Some(format!("{session}/agent-{agent}")),
                _ => Some(session.to_string()),
            }
        });
        parsed
    }
}

fn line(agent: Option<&str>, uuid: &str, role: &str, text: &str) -> String {
    let agent = agent.map_or(String::new(), |a| format!(r#""agentId":"{a}","#));
    format!(
        r#"{{"type":"{role}","uuid":"{uuid}","sessionId":"{PARENT}",{agent}"timestamp":"2026-10-31T18:00:00.000Z","cwd":"/nonexistent/lore-test","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    ) + "\n"
}

/// A parent transcript and one subagent transcript under `subagents/`.
fn transcripts(dir: &Path) -> (PathBuf, PathBuf) {
    let parent = dir.join(format!("{PARENT}.jsonl"));
    std::fs::write(
        &parent,
        line(None, "p1", "user", "ask a helper") + &line(None, "p2", "assistant", "parent heron"),
    )
    .unwrap();
    let subagents = dir.join(PARENT).join("subagents");
    std::fs::create_dir_all(&subagents).unwrap();
    let subagent = subagents.join(format!("agent-{AGENT}.jsonl"));
    std::fs::write(
        &subagent,
        line(Some(AGENT), "s1", "user", "survey")
            + &line(Some(AGENT), "s2", "assistant", "helper otter"),
    )
    .unwrap();
    (parent, subagent)
}

fn ingest(conn: &Connection, adapter: &Keyed, path: &Path, blobs: &BlobStore) {
    let outcome = ingest_file(conn, adapter, path, blobs).unwrap();
    assert!(matches!(outcome, IngestOutcome::Ingested { .. }));
}

/// Mark every source as parsed by an older parser, so the next ingest parses
/// it again with the keying that adapter has.
fn age_parser_version(conn: &Connection) {
    conn.execute("UPDATE ingest_state SET parser_version = '0'", [])
        .unwrap();
}

fn session(conn: &Connection, native_session_id: &str) -> String {
    conn.query_row(
        "SELECT id FROM agent_session WHERE native_session_id = ?1",
        [native_session_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn native_ids(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT native_session_id FROM agent_session ORDER BY native_session_id")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<String>>>()
        .unwrap()
}

fn folder_of(conn: &Connection, native_session_id: &str) -> Option<String> {
    lore_core::folders::folder_of_session(conn, &session(conn, native_session_id)).unwrap()
}

#[test]
fn a_deleted_session_hands_its_folder_to_the_transcript_that_left_it() {
    let dir = tempfile::tempdir().unwrap();
    let (_parent, subagent) = transcripts(dir.path());
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();
    let conn = lore_core::storage::open_in_memory().unwrap();

    ingest(&conn, &Keyed::fold(), &subagent, &blobs);
    let folder = lore_core::folders::create_folder(&conn, "Kept").unwrap();
    lore_core::folders::set_session_folder(&conn, &session(&conn, PARENT), Some(&folder.id))
        .unwrap();
    let added_at: i64 = conn
        .query_row("SELECT added_at FROM session_folder", [], |r| r.get(0))
        .unwrap();

    age_parser_version(&conn);
    ingest(&conn, &Keyed::split(), &subagent, &blobs);
    let sub_id = format!("{PARENT}/agent-{AGENT}");
    assert_eq!(
        native_ids(&conn),
        vec![sub_id.clone()],
        "the session the subagent left has no source and is gone"
    );
    assert_eq!(folder_of(&conn, &sub_id), Some(folder.id));
    let kept_added_at: i64 = conn
        .query_row("SELECT added_at FROM session_folder", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept_added_at, added_at);
}

#[test]
fn a_surviving_parent_keeps_its_folder_and_its_subagent_gets_none() {
    let dir = tempfile::tempdir().unwrap();
    let (parent, subagent) = transcripts(dir.path());
    for order in [
        [parent.clone(), subagent.clone()],
        [subagent.clone(), parent.clone()],
    ] {
        let blob_dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(blob_dir.path()).unwrap();
        let conn = lore_core::storage::open_in_memory().unwrap();
        for path in &order {
            ingest(&conn, &Keyed::fold(), path, &blobs);
        }
        assert_eq!(native_ids(&conn), vec![PARENT.to_string()]);
        let folder = lore_core::folders::create_folder(&conn, "Parent").unwrap();
        lore_core::folders::set_session_folder(&conn, &session(&conn, PARENT), Some(&folder.id))
            .unwrap();

        age_parser_version(&conn);
        for path in &order {
            ingest(&conn, &Keyed::split(), path, &blobs);
        }
        let sub_id = format!("{PARENT}/agent-{AGENT}");
        assert_eq!(native_ids(&conn), vec![PARENT.to_string(), sub_id.clone()]);
        assert_eq!(folder_of(&conn, PARENT), Some(folder.id));
        assert_eq!(
            folder_of(&conn, &sub_id),
            None,
            "filing a parent must not file its subagent"
        );
    }
}
