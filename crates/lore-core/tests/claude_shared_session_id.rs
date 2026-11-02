//! Claude Code writes several transcript files that carry one `sessionId`: a
//! subagent transcript under `<session>/subagents/` repeats its parent's id, and
//! a resumed transcript opens with lines copied from the session it resumed.
//! Ingesting any of those files must not erase the text another one stored.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::{
    AgentAdapter, AgentId, AgentMetadata, Capabilities, Detection, DiscoveryRoots, SessionRef,
};
use lore_core::ingest::{ingest_file, ChangeClass, IngestOutcome};
use lore_core::model::ParsedSession;
use lore_core::search::search;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

const PARENT_ID: &str = "cccccccc-0000-4000-8000-000000000001";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/claude_code/subagent_fold")
}

fn ingest_in_order(paths: &[PathBuf]) -> Connection {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();
    for path in paths {
        ingest_file(&conn, &ClaudeCodeAdapter::new(), path, &blobs).unwrap();
    }
    conn
}

fn session_for(conn: &Connection, native_session_id: &str) -> (String, Option<String>) {
    conn.query_row(
        "SELECT id, title FROM agent_session WHERE native_session_id = ?1",
        [native_session_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

fn part_texts(conn: &Connection, session_id: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT p.text FROM message_part p JOIN message m ON m.id = p.message_id
             WHERE m.session_id = ?1 AND p.text IS NOT NULL ORDER BY m.seq, p.ordinal",
        )
        .unwrap();
    stmt.query_map([session_id], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<String>>>()
        .unwrap()
}

fn hit_sessions(conn: &Connection, term: &str) -> Vec<String> {
    search(conn, term, 20)
        .unwrap()
        .into_iter()
        .map(|h| h.session_id)
        .collect()
}

#[test]
fn subagent_transcript_does_not_replace_its_parent_session() {
    let parent = fixture_dir().join(format!("{PARENT_ID}.jsonl"));
    let subagent = fixture_dir().join(format!(
        "{PARENT_ID}/subagents/agent-a0000000000000001.jsonl"
    ));

    for order in [
        [parent.clone(), subagent.clone()],
        [subagent.clone(), parent.clone()],
    ] {
        let conn = ingest_in_order(&order);
        let (parent_session, title) = session_for(&conn, PARENT_ID);

        assert!(
            part_texts(&conn, &parent_session)
                .contains(&"The parent answer names the heron.".to_string()),
            "parent assistant text must reach message_part.text"
        );
        assert_eq!(title.as_deref(), Some("Placeholder parent title"));
        assert_eq!(hit_sessions(&conn, "heron"), vec![parent_session.clone()]);

        let otter = hit_sessions(&conn, "otter");
        assert_eq!(otter.len(), 1, "subagent text must stay searchable");
        assert_ne!(otter[0], parent_session);
    }
}

fn line(session: &str, uuid: &str, role: &str, text: &str) -> String {
    format!(
        r#"{{"type":"{role}","uuid":"{uuid}","sessionId":"{session}","timestamp":"2026-10-31T18:00:00.000Z","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    ) + "\n"
}

#[test]
fn resumed_transcript_does_not_replace_the_session_it_resumed() {
    let old_id = "dddddddd-0000-4000-8000-000000000001";
    let new_id = "dddddddd-0000-4000-8000-000000000002";
    let copied = line(old_id, "d1", "user", "first question")
        + &line(old_id, "d2", "assistant", "first answer");
    let original = copied.clone() + &line(old_id, "d3", "assistant", "late answer names the cedar");
    let resumed = copied
        + &line(new_id, "d4", "user", "follow up")
        + &line(new_id, "d5", "assistant", "follow up names the birch");

    let dir = tempfile::tempdir().unwrap();
    let original_path = dir.path().join(format!("{old_id}.jsonl"));
    let resumed_path = dir.path().join(format!("{new_id}.jsonl"));
    std::fs::write(&original_path, original).unwrap();
    std::fs::write(&resumed_path, resumed).unwrap();

    for order in [
        [original_path.clone(), resumed_path.clone()],
        [resumed_path.clone(), original_path.clone()],
    ] {
        let conn = ingest_in_order(&order);
        let (original_session, _) = session_for(&conn, old_id);
        let (resumed_session, _) = session_for(&conn, new_id);
        assert_eq!(hit_sessions(&conn, "cedar"), vec![original_session]);
        assert_eq!(hit_sessions(&conn, "birch"), vec![resumed_session]);
    }
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

fn message_count(conn: &Connection, session_id: &str) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM message WHERE session_id = ?1",
        [session_id],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn subagent_session_id_is_the_parent_id_and_agent_file_name() {
    let parent = fixture_dir().join(format!("{PARENT_ID}.jsonl"));
    let subagent = fixture_dir().join(format!(
        "{PARENT_ID}/subagents/agent-a0000000000000001.jsonl"
    ));
    let conn = ingest_in_order(&[parent, subagent]);
    assert_eq!(
        native_ids(&conn),
        vec![
            PARENT_ID.to_string(),
            format!("{PARENT_ID}/agent-a0000000000000001"),
        ]
    );
}

#[test]
fn subagent_without_agent_id_keeps_its_own_session() {
    let parent_id = "eeeeeeee-0000-4000-8000-000000000001";
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join(format!("{parent_id}.jsonl"));
    let sub_dir = dir.path().join(parent_id).join("subagents");
    std::fs::create_dir_all(&sub_dir).unwrap();
    let subagent = sub_dir.join("agent-a0000000000000002.jsonl");
    std::fs::write(
        &parent,
        line(parent_id, "e1", "user", "parent question")
            + &line(
                parent_id,
                "e2",
                "assistant",
                "parent answer names the maple",
            ),
    )
    .unwrap();
    // No agentId on any line: the id must come from the file name.
    std::fs::write(&subagent, line(parent_id, "e3", "user", "subagent prompt")).unwrap();

    for order in [
        [parent.clone(), subagent.clone()],
        [subagent.clone(), parent.clone()],
    ] {
        let conn = ingest_in_order(&order);
        assert_eq!(
            native_ids(&conn),
            vec![
                parent_id.to_string(),
                format!("{parent_id}/agent-a0000000000000002"),
            ]
        );
        let (parent_session, _) = session_for(&conn, parent_id);
        assert_eq!(message_count(&conn, &parent_session), 2);
    }
}

#[test]
fn one_stray_session_id_line_does_not_move_the_file() {
    let own_id = "ffffffff-0000-4000-8000-000000000001";
    let stray_id = "ffffffff-0000-4000-8000-000000000099";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{own_id}.jsonl"));
    let stray =
        format!(r#"{{"type":"last-prompt","sessionId":"{stray_id}","lastPrompt":"x"}}"#) + "\n";
    std::fs::write(
        &path,
        line(own_id, "f1", "user", "question")
            + &line(own_id, "f2", "assistant", "answer names the spruce")
            + &stray,
    )
    .unwrap();
    let conn = ingest_in_order(&[path]);
    assert_eq!(native_ids(&conn), vec![own_id.to_string()]);
}

/// A resumed transcript scanned while it still holds only lines copied from
/// the session it resumed must not take over that session.
#[test]
fn resumed_transcript_with_only_copied_lines_leaves_the_original_whole() {
    let old_id = "abababab-0000-4000-8000-000000000001";
    let new_id = "abababab-0000-4000-8000-000000000002";
    let copied = line(old_id, "g1", "user", "first question")
        + &line(old_id, "g2", "assistant", "first answer names the walnut");
    let original = copied.clone()
        + &line(old_id, "g3", "user", "second question")
        + &line(old_id, "g4", "assistant", "second answer names the juniper");
    let dir = tempfile::tempdir().unwrap();
    let original_path = dir.path().join(format!("{old_id}.jsonl"));
    let resumed_path = dir.path().join(format!("{new_id}.jsonl"));
    std::fs::write(&original_path, original).unwrap();
    std::fs::write(&resumed_path, &copied).unwrap();

    let conn = lore_core::storage::open_in_memory().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();
    let adapter = ClaudeCodeAdapter::new();
    ingest_file(&conn, &adapter, &original_path, &blobs).unwrap();
    ingest_file(&conn, &adapter, &resumed_path, &blobs).unwrap();
    let (original_session, _) = session_for(&conn, old_id);
    assert_eq!(message_count(&conn, &original_session), 4);
    assert_eq!(
        hit_sessions(&conn, "juniper"),
        vec![original_session.clone()]
    );

    // The resumed file then gains its own lines.
    std::fs::write(
        &resumed_path,
        copied
            + &line(
                new_id,
                "g5",
                "assistant",
                "resumed answer names the sequoia",
            ),
    )
    .unwrap();
    ingest_file(&conn, &adapter, &resumed_path, &blobs).unwrap();
    assert_eq!(message_count(&conn, &original_session), 4);
    let (resumed_session, _) = session_for(&conn, new_id);
    assert_eq!(message_count(&conn, &resumed_session), 3);
    assert_eq!(
        native_ids(&conn),
        vec![old_id.to_string(), new_id.to_string()]
    );
}

/// The subagent fixture tree copied somewhere writable.
fn copy_fixture_tree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join(format!("{PARENT_ID}.jsonl"));
    let sub_dir = dir.path().join(PARENT_ID).join("subagents");
    std::fs::create_dir_all(&sub_dir).unwrap();
    let subagent = sub_dir.join("agent-a0000000000000001.jsonl");
    std::fs::copy(fixture_dir().join(format!("{PARENT_ID}.jsonl")), &parent).unwrap();
    std::fs::copy(
        fixture_dir().join(format!(
            "{PARENT_ID}/subagents/agent-a0000000000000001.jsonl"
        )),
        &subagent,
    )
    .unwrap();
    (dir, parent, subagent)
}

#[test]
fn an_appended_subagent_transcript_keeps_its_id_and_grows() {
    let (_dir, parent, subagent) = copy_fixture_tree();
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();
    let adapter = ClaudeCodeAdapter::new();
    ingest_file(&conn, &adapter, &parent, &blobs).unwrap();
    ingest_file(&conn, &adapter, &subagent, &blobs).unwrap();
    let sub_id = format!("{PARENT_ID}/agent-a0000000000000001");
    let (sub_session, _) = session_for(&conn, &sub_id);
    assert_eq!(message_count(&conn, &sub_session), 2);

    let tail = format!(
        r#"{{"type":"assistant","uuid":"c5555555-5555-4555-8555-555555555555","sessionId":"{PARENT_ID}","agentId":"a0000000000000001","timestamp":"2026-10-31T18:00:09.000Z","message":{{"role":"assistant","content":[{{"type":"text","text":"The subagent adds the badger."}}]}}}}"#
    ) + "\n";
    let mut content = std::fs::read_to_string(&subagent).unwrap();
    content.push_str(&tail);
    std::fs::write(&subagent, content).unwrap();

    let outcome = ingest_file(&conn, &adapter, &subagent, &blobs).unwrap();
    assert!(
        matches!(
            outcome,
            IngestOutcome::Ingested {
                change: ChangeClass::Appended,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(
        native_ids(&conn),
        vec![PARENT_ID.to_string(), sub_id.clone()]
    );
    assert_eq!(session_for(&conn, &sub_id).0, sub_session);
    assert_eq!(message_count(&conn, &sub_session), 3);
    assert_eq!(hit_sessions(&conn, "badger"), vec![sub_session]);
    let (parent_session, _) = session_for(&conn, PARENT_ID);
    assert_eq!(message_count(&conn, &parent_session), 2);
}

/// The Claude adapter as parser v3 keyed a file: by the first `sessionId` it
/// carries, so a subagent transcript took its parent's session.
struct V3Claude(ClaudeCodeAdapter);

impl AgentAdapter for V3Claude {
    fn id(&self) -> AgentId {
        self.0.id()
    }
    fn metadata(&self) -> AgentMetadata {
        self.0.metadata()
    }
    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }
    fn detect_installation(&self, roots: &DiscoveryRoots) -> Detection {
        self.0.detect_installation(roots)
    }
    fn roots(&self, overrides: &DiscoveryRoots) -> Vec<PathBuf> {
        self.0.roots(overrides)
    }
    fn discover_sessions(&self, roots: &DiscoveryRoots) -> Vec<SessionRef> {
        self.0.discover_sessions(roots)
    }
    fn parse_content(&self, content: &str, fallback_dedupe: &str) -> ParsedSession {
        let mut parsed = self.0.parse_content(content, fallback_dedupe);
        parsed.native_session_id = content.lines().find_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            v.get("sessionId")?.as_str().map(str::to_string)
        });
        parsed
    }
}

/// (native id, message count, title) of every session.
fn sessions(conn: &Connection) -> Vec<(String, i64, Option<String>)> {
    let mut stmt = conn
        .prepare(
            "SELECT native_session_id, message_count, title FROM agent_session
             ORDER BY native_session_id",
        )
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

#[test]
fn upgrading_a_v3_archive_matches_a_fresh_scan_and_keeps_the_folder() {
    let (_dir, parent, subagent) = copy_fixture_tree();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();

    let archive = lore_core::storage::open_in_memory().unwrap();
    let v3 = V3Claude(ClaudeCodeAdapter::new());
    ingest_file(&archive, &v3, &parent, &blobs).unwrap();
    ingest_file(&archive, &v3, &subagent, &blobs).unwrap();
    archive
        .execute("UPDATE ingest_state SET parser_version = '3'", [])
        .unwrap();
    let v3_sessions = sessions(&archive);
    assert_eq!(
        v3_sessions.len(),
        1,
        "v3 folds the subagent into its parent"
    );
    let (parent_session, _) = session_for(&archive, PARENT_ID);
    let folder = lore_core::folders::create_folder(&archive, "Kept").unwrap();
    lore_core::folders::set_session_folder(&archive, &parent_session, Some(&folder.id)).unwrap();

    let adapter = ClaudeCodeAdapter::new();
    for path in [&parent, &subagent] {
        ingest_file(&archive, &adapter, path, &blobs).unwrap();
    }
    let fresh = ingest_in_order(&[parent.clone(), subagent.clone()]);
    assert_eq!(sessions(&archive), sessions(&fresh));
    assert_eq!(
        sessions(&archive),
        vec![
            (
                PARENT_ID.to_string(),
                2,
                Some("Placeholder parent title".to_string())
            ),
            (format!("{PARENT_ID}/agent-a0000000000000001"), 2, None),
        ]
    );
    assert_eq!(session_for(&archive, PARENT_ID).0, parent_session);
    assert_eq!(
        lore_core::folders::folder_of_session(&archive, &parent_session).unwrap(),
        Some(folder.id)
    );
    let orphans: i64 = archive
        .query_row(
            "SELECT count(*) FROM source_artifact sa WHERE NOT EXISTS
             (SELECT 1 FROM session_source ss WHERE ss.source_artifact_id = sa.id)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0);
}
