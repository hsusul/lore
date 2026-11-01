//! Claude Code writes several transcript files that carry one `sessionId`: a
//! subagent transcript under `<session>/subagents/` repeats its parent's id, and
//! a resumed transcript opens with lines copied from the session it resumed.
//! Ingesting any of those files must not erase the text another one stored.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::ingest::ingest_file;
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
