//! Read queries that project canonical rows into IPC DTOs for the UI.
//!
//! Heavy work stays in the Rust core (`AGENTS.md`): the webview never runs SQL.
//! These functions back the first M0 commands — `list_detected_agents` and
//! `list_sessions` — returning [`lore_ipc`] wire types directly.

use lore_ipc::{DetectedAgent, RepositorySummary, SessionSummary};
use rusqlite::Connection;

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
    use crate::ingest::persist_session;
    use crate::storage::blob::BlobStore;

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
}
