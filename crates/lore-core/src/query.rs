//! Read queries that project canonical rows into IPC DTOs for the UI.
//!
//! Heavy work stays in the Rust core (`AGENTS.md`): the webview never runs SQL.
//! These functions back the read commands — `list_detected_agents`,
//! `list_sessions`, `list_repositories`, `get_session`, and `get_git_snapshot`
//! — returning [`lore_ipc`] wire types directly. Opaque/encrypted parts are
//! returned without readable content and are never rendered or exported.

use std::collections::HashMap;

use lore_ipc::{
    DetectedAgent, FileEventDto, GitObservationDto, MessageDto, MessagePartDto, RepositorySummary,
    SegmentDto, SessionDetail, SessionSummary,
};
use rusqlite::{Connection, OptionalExtension};

use crate::storage::Result;

/// The agents Lore knows about, with their ingested-session counts, ordered by
/// id for a stable list.
pub fn list_agents(conn: &Connection) -> Result<Vec<DetectedAgent>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.display_name, a.detected, a.version,
                (SELECT count(*) FROM agent_session s WHERE s.agent_id = a.id)
         FROM agent a
         ORDER BY a.id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DetectedAgent {
                id: row.get(0)?,
                display_name: row.get(1)?,
                installed: row.get::<_, i64>(2)? != 0,
                version: row.get(3)?,
                session_count: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The most recent sessions (newest first), capped at `limit`. A stable
/// `(started_at, id)` order keeps this ready for keyset pagination later.
pub fn list_sessions(conn: &Connection, limit: i64) -> Result<Vec<SessionSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, title, started_at, ended_at, message_count,
                tool_call_count, primary_model, parse_status
         FROM agent_session
         ORDER BY started_at DESC, id DESC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok(SessionSummary {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                title: row.get(2)?,
                started_at: row.get(3)?,
                ended_at: row.get(4)?,
                message_count: row.get(5)?,
                tool_call_count: row.get(6)?,
                primary_model: row.get(7)?,
                parse_status: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The full read of one session: header, context segments, the ordered-part
/// message timeline, and touched files. Returns `None` when the session is
/// unknown. Opaque/encrypted parts are returned without readable content and
/// are never rendered or exported.
pub fn get_session(conn: &Connection, session_id: &str) -> Result<Option<SessionDetail>> {
    let Some(summary) = session_summary(conn, session_id)? else {
        return Ok(None);
    };
    let segments = session_segments(conn, session_id)?;
    let messages = session_messages(conn, session_id)?;
    let file_events = session_file_events(conn, session_id)?;
    Ok(Some(SessionDetail {
        summary,
        segments,
        messages,
        file_events,
    }))
}

fn session_summary(conn: &Connection, session_id: &str) -> Result<Option<SessionSummary>> {
    conn.query_row(
        "SELECT id, agent_id, title, started_at, ended_at, message_count,
                tool_call_count, primary_model, parse_status
         FROM agent_session WHERE id = ?1",
        [session_id],
        |row| {
            Ok(SessionSummary {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                title: row.get(2)?,
                started_at: row.get(3)?,
                ended_at: row.get(4)?,
                message_count: row.get(5)?,
                tool_call_count: row.get(6)?,
                primary_model: row.get(7)?,
                parse_status: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn session_segments(conn: &Connection, session_id: &str) -> Result<Vec<SegmentDto>> {
    let mut stmt = conn.prepare(
        "SELECT id, seq_start, seq_end, cwd, model, provider, repository_id,
                resolution_confidence
         FROM session_segment WHERE session_id = ?1 ORDER BY seq_start",
    )?;
    let rows = stmt
        .query_map([session_id], |row| {
            Ok(SegmentDto {
                id: row.get(0)?,
                seq_start: row.get(1)?,
                seq_end: row.get(2)?,
                cwd: row.get(3)?,
                model: row.get(4)?,
                provider: row.get(5)?,
                repository_id: row.get(6)?,
                resolution_confidence: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn session_messages(conn: &Connection, session_id: &str) -> Result<Vec<MessageDto>> {
    // Parts are fetched once for the whole session and grouped by message seq to
    // avoid an N+1 query. Opaque parts are stripped of readable content here.
    let mut parts_by_seq: HashMap<i64, Vec<MessagePartDto>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT m.seq, mp.ordinal, mp.kind, mp.text, mp.content_json, mp.searchable
             FROM message_part mp JOIN message m ON m.id = mp.message_id
             WHERE m.session_id = ?1
             ORDER BY m.seq, mp.ordinal",
        )?;
        let mut rows = stmt.query([session_id])?;
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let kind: String = row.get(2)?;
            let opaque = kind == "opaque";
            parts_by_seq.entry(seq).or_default().push(MessagePartDto {
                ordinal: row.get(1)?,
                kind,
                text: if opaque { None } else { row.get(3)? },
                content_json: if opaque { None } else { row.get(4)? },
                searchable: row.get::<_, i64>(5)? != 0,
            });
        }
    }

    let mut stmt = conn.prepare(
        "SELECT id, seq, role, event_kind, is_sidechain, ts, model
         FROM message WHERE session_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt
        .query_map([session_id], |row| {
            let seq: i64 = row.get(1)?;
            Ok(MessageDto {
                id: row.get(0)?,
                seq,
                role: row.get(2)?,
                event_kind: row.get(3)?,
                is_sidechain: row.get::<_, i64>(4)? != 0,
                ts: row.get(5)?,
                model: row.get(6)?,
                parts: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(rows
        .into_iter()
        .map(|mut message| {
            message.parts = parts_by_seq.remove(&message.seq).unwrap_or_default();
            message
        })
        .collect())
}

fn session_file_events(conn: &Connection, session_id: &str) -> Result<Vec<FileEventDto>> {
    let mut stmt = conn.prepare(
        "SELECT path, change_kind, old_path, lines_added, lines_removed, source,
                patch_blob_id IS NOT NULL
         FROM file_event WHERE session_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map([session_id], |row| {
            Ok(FileEventDto {
                path: row.get(0)?,
                change_kind: row.get(1)?,
                old_path: row.get(2)?,
                lines_added: row.get(3)?,
                lines_removed: row.get(4)?,
                source: row.get(5)?,
                has_patch: row.get::<_, i64>(6)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every git observation for a session, provenance-labeled and ordered by
/// observation time then source, for the session's git rail. Distinct sources
/// (agent-recorded, agent-patch, Lore-captured, Lore-reverified) coexist and are
/// never merged.
pub fn get_git_snapshot(conn: &Connection, session_id: &str) -> Result<Vec<GitObservationDto>> {
    let mut stmt = conn.prepare(
        "SELECT segment_id, source, event_ts, observed_at, temporal_confidence,
                branch, commit_sha, remote_url_norm, is_dirty, commit_exists
         FROM git_observation
         WHERE session_id = ?1
         ORDER BY observed_at, source, id",
    )?;
    let rows = stmt
        .query_map([session_id], |row| {
            Ok(GitObservationDto {
                segment_id: row.get(0)?,
                source: row.get(1)?,
                event_ts: row.get(2)?,
                observed_at: row.get(3)?,
                temporal_confidence: row.get(4)?,
                branch: row.get(5)?,
                commit_sha: row.get(6)?,
                remote_url_norm: row.get(7)?,
                is_dirty: row.get::<_, Option<i64>>(8)?.map(|v| v != 0),
                commit_exists: row.get::<_, Option<i64>>(9)?.map(|v| v != 0),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Repositories resolved by git enrichment, with session and worktree counts,
/// ordered by display name for a stable list.
pub fn list_repositories(conn: &Connection) -> Result<Vec<RepositorySummary>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.display_name, r.identity_confidence, r.primary_path, r.is_missing,
                (SELECT count(DISTINCT sg.session_id) FROM session_segment sg
                 WHERE sg.repository_id = r.id),
                (SELECT count(*) FROM worktree w WHERE w.repository_id = r.id)
         FROM repository r
         ORDER BY r.display_name, r.id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RepositorySummary {
                id: row.get(0)?,
                display_name: row.get(1)?,
                identity_confidence: row.get(2)?,
                primary_path: row.get(3)?,
                is_missing: row.get::<_, i64>(4)? != 0,
                session_count: row.get(5)?,
                worktree_count: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::claude_code::ClaudeCodeAdapter;
    use crate::adapters::codex::CodexAdapter;
    use crate::ingest::persist_session;
    use crate::storage::blob::BlobStore;

    fn fixture(dir: &str, name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(dir)
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    fn seed(conn: &Connection) {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let content = concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"q1\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"sessionId\":\"q1\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n"
        );
        let parsed = ClaudeCodeAdapter::new().parse_str(content, "q1");
        persist_session(conn, "claude-code", "Claude Code", &parsed, &blobs).unwrap();
    }

    #[test]
    fn lists_agents_with_session_counts() {
        let conn = crate::storage::open_in_memory().unwrap();
        seed(&conn);
        let agents = list_agents(&conn).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "claude-code");
        assert_eq!(agents[0].display_name, "Claude Code");
        assert!(agents[0].installed);
        assert_eq!(agents[0].session_count, 1);
    }

    #[test]
    fn lists_sessions_newest_first() {
        let conn = crate::storage::open_in_memory().unwrap();
        seed(&conn);
        let sessions = list_sessions(&conn, 50).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent_id, "claude-code");
        assert_eq!(sessions[0].message_count, 2);
        assert_eq!(sessions[0].parse_status, "ok");
    }

    #[test]
    fn empty_database_lists_nothing() {
        let conn = crate::storage::open_in_memory().unwrap();
        assert!(list_agents(&conn).unwrap().is_empty());
        assert!(list_sessions(&conn, 10).unwrap().is_empty());
        assert!(list_repositories(&conn).unwrap().is_empty());
    }

    #[test]
    fn get_session_returns_ordered_timeline_and_file_events() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let parsed =
            ClaudeCodeAdapter::new().parse_str(&fixture("claude_code", "tool_use.jsonl"), "tu");
        let sid = persist_session(&conn, "claude-code", "Claude Code", &parsed, &blobs).unwrap();

        let detail = get_session(&conn, &sid).unwrap().expect("session exists");
        assert_eq!(detail.summary.id, sid);
        assert!(!detail.messages.is_empty());
        // Messages are in strict source order.
        let seqs: Vec<i64> = detail.messages.iter().map(|m| m.seq).collect();
        assert!(
            seqs.windows(2).all(|w| w[0] < w[1]),
            "messages ordered by seq"
        );
        // Each message's parts are ordinal-ordered.
        for message in &detail.messages {
            let ords: Vec<i64> = message.parts.iter().map(|p| p.ordinal).collect();
            assert!(ords.windows(2).all(|w| w[0] < w[1]));
        }
        // The Edit tool produced one agent_tool_input file event with no patch blob.
        assert_eq!(detail.file_events.len(), 1);
        assert_eq!(detail.file_events[0].source, "agent_tool_input");
        assert_eq!(detail.file_events[0].change_kind, "edit");
        assert!(!detail.file_events[0].has_patch);

        assert!(get_session(&conn, "missing").unwrap().is_none());
    }

    #[test]
    fn get_session_marks_thinking_nonsearchable_and_redacts_opaque() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();

        // Claude: assistant turn with thinking then text.
        let claude =
            ClaudeCodeAdapter::new().parse_str(&fixture("claude_code", "basic_text.jsonl"), "b");
        let csid = persist_session(&conn, "claude-code", "Claude Code", &claude, &blobs).unwrap();
        let detail = get_session(&conn, &csid).unwrap().unwrap();
        let assistant = detail
            .messages
            .iter()
            .find(|m| m.role == "assistant")
            .unwrap();
        let thinking = assistant
            .parts
            .iter()
            .find(|p| p.kind == "thinking")
            .unwrap();
        assert!(!thinking.searchable, "thinking is not searchable");
        assert!(
            thinking.text.is_some(),
            "thinking is still locally viewable"
        );

        // Codex: encrypted reasoning must come back with no readable content.
        let content = "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"type\":\"reasoning\",\"summary\":\"plan\",\"encrypted_content\":\"SECRET-BLOB\"}}\n";
        let codex = CodexAdapter::new().parse_str(content, "enc");
        let xsid = persist_session(&conn, "codex", "Codex", &codex, &blobs).unwrap();
        let xdetail = get_session(&conn, &xsid).unwrap().unwrap();
        let opaque = xdetail
            .messages
            .iter()
            .flat_map(|m| &m.parts)
            .find(|p| p.kind == "opaque")
            .expect("opaque part present");
        assert!(opaque.text.is_none() && opaque.content_json.is_none());
        assert!(!opaque.searchable);
    }
}
