//! Read queries that project canonical rows into IPC DTOs for the UI.
//!
//! Heavy work stays in the Rust core (`AGENTS.md`): the webview never runs SQL.
//! These functions back the archive read commands — `list_sessions`,
//! `list_repositories`, `get_session`, and `get_git_snapshot`
//! — returning [`lore_ipc`] wire types directly. Opaque/encrypted parts are
//! returned without readable content and are never rendered or exported.

use std::collections::HashMap;

use lore_ipc::{
    FileEventDto, GitObservationDto, MessageDto, MessagePage, MessagePartDto, RepositorySummary,
    SegmentDto, SessionDetail, SessionPage, SessionSummary, TokenTotalsDto,
};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension};

use crate::storage::blob::BlobStore;
use crate::storage::Result;

/// Calculate cumulative token usage and estimated cost across the entire archive.
pub fn get_token_totals(conn: &Connection) -> Result<TokenTotalsDto> {
    conn.query_row(
        "SELECT COALESCE(SUM(total_input_tokens), 0),
                COALESCE(SUM(total_output_tokens), 0),
                COALESCE(SUM(total_cache_tokens), 0),
                COALESCE(SUM(est_cost_usd), 0.0)
         FROM agent_session",
        [],
        |row| {
            Ok(TokenTotalsDto {
                total_input_tokens: row.get(0)?,
                total_output_tokens: row.get(1)?,
                total_cache_tokens: row.get(2)?,
                est_cost_usd: row.get(3)?,
            })
        },
    )
    .map_err(Into::into)
}

/// Number of sessions already archived for one adapter.
pub fn agent_session_count(conn: &Connection, agent_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM agent_session WHERE agent_id = ?1",
        [agent_id],
        |row| row.get(0),
    )?)
}

/// The most recent sessions (newest first), capped at `limit`. A stable
/// `(started_at, id)` order keeps this ready for keyset pagination later.
pub fn list_sessions(conn: &Connection, limit: i64) -> Result<Vec<SessionSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, title, started_at, ended_at, message_count,
                tool_call_count, primary_model, parse_status,
                total_input_tokens, total_output_tokens, total_cache_tokens, est_cost_usd
         FROM agent_session
         ORDER BY started_at DESC, id DESC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], session_summary_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Map the standard session-summary projection (columns 0..=12, in
/// the field order every `SELECT` in this module uses). Shared so the column
/// order lives in exactly one place.
fn session_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummary> {
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
        total_input_tokens: row.get(9)?,
        total_output_tokens: row.get(10)?,
        total_cache_tokens: row.get(11)?,
        est_cost_usd: row.get(12)?,
    })
}

/// The keyset predicate body (no leading `WHERE`/`AND`) keeping only rows
/// strictly after `cursor` in the newest-first `(started_at DESC, id DESC)`
/// total order. `prefix` qualifies the columns (`""` or `"s."`). Missing
/// timestamps sort last, so a null-`started_at` cursor is confined to that
/// trailing block. Shared by [`list_sessions_page`],
/// [`list_repository_sessions_page`], and [`list_folder_sessions_page`] so the
/// pages cannot drift apart.
fn keyset_after(cursor: &SessionCursor, prefix: &str) -> (&'static str, Vec<Value>) {
    let params = match cursor.started_at {
        Some(started_at) => vec![
            Value::Integer(started_at),
            Value::Integer(started_at),
            Value::Text(cursor.id.clone()),
        ],
        None => vec![Value::Text(cursor.id.clone())],
    };
    let clause = match (cursor.started_at.is_some(), prefix) {
        (true, "s.") => {
            "s.started_at < ? OR s.started_at IS NULL OR (s.started_at = ? AND s.id < ?)"
        }
        (true, _) => "started_at < ? OR started_at IS NULL OR (started_at = ? AND id < ?)",
        (false, "s.") => "s.started_at IS NULL AND s.id < ?",
        (false, _) => "started_at IS NULL AND id < ?",
    };
    (clause, params)
}

/// List one stable newest-first page of sessions. The opaque cursor stores the
/// final row's `(started_at, id)` key, so later pages do not pay an OFFSET cost
/// and cannot repeat or skip rows in an unchanged archive. Missing timestamps
/// sort after timestamped sessions, matching [`list_sessions`]. A malformed
/// cursor safely degrades to the first page.
pub fn list_sessions_page(
    conn: &Connection,
    limit: i64,
    cursor: Option<&str>,
) -> Result<SessionPage> {
    let limit = limit.clamp(1, 10_000);
    let cursor = cursor.and_then(SessionCursor::decode);
    let mut sql = String::with_capacity(384);
    sql.push_str(
        "SELECT id, agent_id, title, started_at, ended_at, message_count,
                tool_call_count, primary_model, parse_status,
                total_input_tokens, total_output_tokens, total_cache_tokens, est_cost_usd
         FROM agent_session",
    );
    let mut params = Vec::with_capacity(8);
    if let Some(cursor) = &cursor {
        let (body, keyset_params) = keyset_after(cursor, "");
        sql.push_str(" WHERE ");
        sql.push_str(body);
        params.extend(keyset_params);
    }
    sql.push_str(" ORDER BY started_at DESC, id DESC LIMIT ?");
    params.push(Value::Integer(limit + 1));
    query_session_page(conn, &sql, params, limit)
}

/// The most recent sessions that touched `repository_id` (newest first), capped
/// at `limit`. A session qualifies if any of its segments resolved to the repo.
pub fn list_repository_sessions(
    conn: &Connection,
    repository_id: &str,
    limit: i64,
) -> Result<Vec<SessionSummary>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.agent_id, s.title, s.started_at, s.ended_at, s.message_count,
                s.tool_call_count, s.primary_model, s.parse_status,
                s.total_input_tokens, s.total_output_tokens, s.total_cache_tokens, s.est_cost_usd
         FROM agent_session s
         WHERE EXISTS (
             SELECT 1 FROM session_segment sg
             WHERE sg.session_id = s.id AND sg.repository_id = ?1
         )
         ORDER BY s.started_at DESC, s.id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![repository_id, limit], session_summary_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Paginated counterpart of [`list_repository_sessions`], using the same
/// newest-first total order and opaque keyset cursor as [`list_sessions_page`].
pub fn list_repository_sessions_page(
    conn: &Connection,
    repository_id: &str,
    limit: i64,
    cursor: Option<&str>,
) -> Result<SessionPage> {
    let limit = limit.clamp(1, 10_000);
    let cursor = cursor.and_then(SessionCursor::decode);
    let mut sql = String::with_capacity(512);
    sql.push_str(
        "SELECT s.id, s.agent_id, s.title, s.started_at, s.ended_at, s.message_count,
                s.tool_call_count, s.primary_model, s.parse_status,
                s.total_input_tokens, s.total_output_tokens, s.total_cache_tokens, s.est_cost_usd
         FROM agent_session s
         WHERE EXISTS (
             SELECT 1 FROM session_segment sg
             WHERE sg.session_id = s.id AND sg.repository_id = ?
         )",
    );
    let mut params = Vec::with_capacity(8);
    params.push(Value::Text(repository_id.to_string()));
    if let Some(cursor) = &cursor {
        let (body, keyset_params) = keyset_after(cursor, "s.");
        sql.push_str(" AND (");
        sql.push_str(body);
        sql.push(')');
        params.extend(keyset_params);
    }
    sql.push_str(" ORDER BY s.started_at DESC, s.id DESC LIMIT ?");
    params.push(Value::Integer(limit + 1));
    query_session_page(conn, &sql, params, limit)
}

/// Paginated listing of the threads filed in one folder, newest-first, using
/// the same total order and opaque keyset cursor as [`list_sessions_page`].
/// Folder membership lives in `session_folder` (see [`crate::folders`]).
pub fn list_folder_sessions_page(
    conn: &Connection,
    folder_id: &str,
    limit: i64,
    cursor: Option<&str>,
) -> Result<SessionPage> {
    let limit = limit.clamp(1, 10_000);
    let cursor = cursor.and_then(SessionCursor::decode);
    let mut sql = String::with_capacity(512);
    sql.push_str(
        "SELECT s.id, s.agent_id, s.title, s.started_at, s.ended_at, s.message_count,
                s.tool_call_count, s.primary_model, s.parse_status,
                s.total_input_tokens, s.total_output_tokens, s.total_cache_tokens, s.est_cost_usd
         FROM agent_session s
         JOIN session_folder sf ON sf.session_id = s.id
         WHERE sf.folder_id = ?",
    );
    let mut params = Vec::with_capacity(8);
    params.push(Value::Text(folder_id.to_string()));
    if let Some(cursor) = &cursor {
        let (body, keyset_params) = keyset_after(cursor, "s.");
        sql.push_str(" AND (");
        sql.push_str(body);
        sql.push(')');
        params.extend(keyset_params);
    }
    sql.push_str(" ORDER BY s.started_at DESC, s.id DESC LIMIT ?");
    params.push(Value::Integer(limit + 1));
    query_session_page(conn, &sql, params, limit)
}

fn query_session_page(
    conn: &Connection,
    sql: &str,
    params: Vec<Value>,
    limit: i64,
) -> Result<SessionPage> {
    let mut stmt = conn.prepare(sql)?;
    let mut sessions = stmt
        .query_map(rusqlite::params_from_iter(params), session_summary_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = sessions.len() > limit as usize;
    sessions.truncate(limit as usize);
    let next_cursor = if has_more {
        sessions
            .last()
            .map(|s| SessionCursor::encode_parts(s.started_at, &s.id))
    } else {
        None
    };
    Ok(SessionPage {
        sessions,
        next_cursor,
    })
}

/// Opaque keyset cursor for the browse order. Session ids are percent-escaped
/// so the cursor remains safely round-trippable even if an adapter supplies a
/// native id containing the `:` separator.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionCursor {
    started_at: Option<i64>,
    id: String,
}

impl SessionCursor {
    fn encode_parts(started_at: Option<i64>, id: &str) -> String {
        use std::fmt::Write;
        let mut out = String::with_capacity(id.len() + 24);
        match started_at {
            Some(value) => {
                let _ = write!(&mut out, "{value}:");
            }
            None => out.push_str("n:"),
        }
        for b in id.bytes() {
            match b {
                b'%' => out.push_str("%25"),
                b':' => out.push_str("%3A"),
                _ => out.push(b as char),
            }
        }
        out
    }

    #[cfg(test)]
    fn encode(&self) -> String {
        Self::encode_parts(self.started_at, &self.id)
    }

    fn decode(raw: &str) -> Option<Self> {
        if raw.len() > 2_048 {
            return None;
        }
        let (started_at, encoded_id) = raw.split_once(':')?;
        if encoded_id.is_empty() || encoded_id.contains(':') {
            return None;
        }
        let started_at = match started_at {
            "n" => None,
            value => Some(value.parse().ok()?),
        };
        let mut id = String::with_capacity(encoded_id.len());
        let mut chars = encoded_id.chars();
        while let Some(c) = chars.next() {
            if c == '%' {
                let c1 = chars.next();
                let c2 = chars.next();
                match (c1, c2) {
                    (Some('2'), Some('5')) => id.push('%'),
                    (Some('3'), Some('A' | 'a')) => id.push(':'),
                    (Some(a), Some(b)) => {
                        id.push('%');
                        id.push(a);
                        id.push(b);
                    }
                    (Some(a), None) => {
                        id.push('%');
                        id.push(a);
                    }
                    _ => id.push('%'),
                }
            } else {
                id.push(c);
            }
        }
        Some(Self { started_at, id })
    }
}

/// Number of initial messages returned in [`get_session`].
const INITIAL_MESSAGE_LIMIT: i64 = 200;

/// Keyset cursor for messages in a session, ordered by `seq ASC`.
struct MessageCursor {
    seq: i64,
}

impl MessageCursor {
    fn encode(&self) -> String {
        self.seq.to_string()
    }

    fn decode(s: &str) -> Option<Self> {
        s.parse::<i64>().ok().map(|seq| Self { seq })
    }
}

/// The read of one session in context: header, context segments, the initial
/// page of messages (up to 200), touched files, and a cursor for subsequent
/// message pages when available. Returns `None` when the session is unknown.
/// Opaque/encrypted parts are returned without readable content and are never
/// rendered or exported.
pub fn get_session(conn: &Connection, session_id: &str) -> Result<Option<SessionDetail>> {
    let Some((summary, parse_note)) = session_summary(conn, session_id)? else {
        return Ok(None);
    };
    let segments = session_segments(conn, session_id)?;
    let (messages, has_more) =
        query_session_messages_window(conn, session_id, INITIAL_MESSAGE_LIMIT, None)?;
    let next_message_cursor = if has_more {
        messages
            .last()
            .map(|m| MessageCursor { seq: m.seq }.encode())
    } else {
        None
    };
    let file_events = session_file_events(conn, session_id)?;
    Ok(Some(SessionDetail {
        summary,
        parse_note,
        segments,
        messages,
        file_events,
        next_message_cursor,
    }))
}

/// List one stable page of messages for `session_id`, ordered by `seq ASC`.
/// Used by the UI timeline to stream subsequent message batches without
/// crossing IPC in a single unbounded DTO.
pub fn list_session_messages_page(
    conn: &Connection,
    session_id: &str,
    limit: i64,
    cursor: Option<&str>,
) -> Result<MessagePage> {
    let limit = limit.clamp(1, 1_000);
    let after_seq = cursor.and_then(MessageCursor::decode).map(|c| c.seq);

    let (messages, has_more) = query_session_messages_window(conn, session_id, limit, after_seq)?;
    let next_cursor = if has_more {
        messages
            .last()
            .map(|m| MessageCursor { seq: m.seq }.encode())
    } else {
        None
    };

    Ok(MessagePage {
        messages,
        next_cursor,
    })
}

fn session_summary(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<(SessionSummary, Option<String>)>> {
    conn.query_row(
        "SELECT id, agent_id, title, started_at, ended_at, message_count,
                tool_call_count, primary_model, parse_status,
                total_input_tokens, total_output_tokens, total_cache_tokens, est_cost_usd,
                parse_note
         FROM agent_session WHERE id = ?1",
        [session_id],
        |row| Ok((session_summary_row(row)?, row.get(13)?)),
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

struct RawMessageRow {
    id: String,
    seq: i64,
    role: String,
    event_kind: String,
    is_sidechain: bool,
    ts: Option<i64>,
    model: Option<String>,
}

fn query_session_messages_window(
    conn: &Connection,
    session_id: &str,
    limit: i64,
    after_seq: Option<i64>,
) -> Result<(Vec<MessageDto>, bool)> {
    let fetch_limit = limit.saturating_add(1);

    let mut raw_messages: Vec<RawMessageRow> = Vec::new();
    match after_seq {
        Some(seq) => {
            let mut stmt = conn.prepare(
                "SELECT id, seq, role, event_kind, is_sidechain, ts, model
                 FROM message
                 WHERE session_id = ?1 AND seq > ?2
                 ORDER BY seq LIMIT ?3",
            )?;
            let mut rows = stmt.query(rusqlite::params![session_id, seq, fetch_limit])?;
            while let Some(row) = rows.next()? {
                raw_messages.push(RawMessageRow {
                    id: row.get(0)?,
                    seq: row.get(1)?,
                    role: row.get(2)?,
                    event_kind: row.get(3)?,
                    is_sidechain: row.get::<_, i64>(4)? != 0,
                    ts: row.get(5)?,
                    model: row.get(6)?,
                });
            }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT id, seq, role, event_kind, is_sidechain, ts, model
                 FROM message
                 WHERE session_id = ?1
                 ORDER BY seq LIMIT ?2",
            )?;
            let mut rows = stmt.query(rusqlite::params![session_id, fetch_limit])?;
            while let Some(row) = rows.next()? {
                raw_messages.push(RawMessageRow {
                    id: row.get(0)?,
                    seq: row.get(1)?,
                    role: row.get(2)?,
                    event_kind: row.get(3)?,
                    is_sidechain: row.get::<_, i64>(4)? != 0,
                    ts: row.get(5)?,
                    model: row.get(6)?,
                });
            }
        }
    };

    let has_more = raw_messages.len() as i64 > limit;
    if has_more {
        raw_messages.truncate(limit as usize);
    }

    if raw_messages.is_empty() {
        return Ok((Vec::new(), false));
    }

    let min_seq = raw_messages.first().map(|m| m.seq).unwrap_or(0);
    let max_seq = raw_messages.last().map(|m| m.seq).unwrap_or(0);

    let mut parts_by_seq: HashMap<i64, Vec<MessagePartDto>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT m.seq, mp.ordinal, mp.kind, mp.text, mp.content_json, mp.searchable
             FROM message_part mp JOIN message m ON m.id = mp.message_id
             WHERE m.session_id = ?1 AND m.seq >= ?2 AND m.seq <= ?3
             ORDER BY m.seq, mp.ordinal",
        )?;
        let mut rows = stmt.query(rusqlite::params![session_id, min_seq, max_seq])?;
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

    let messages = raw_messages
        .into_iter()
        .map(|row| {
            let parts = parts_by_seq.remove(&row.seq).unwrap_or_default();
            MessageDto {
                id: row.id,
                seq: row.seq,
                role: row.role,
                event_kind: row.event_kind,
                is_sidechain: row.is_sidechain,
                ts: row.ts,
                model: row.model,
                parts,
            }
        })
        .collect();

    Ok((messages, has_more))
}

fn session_file_events(conn: &Connection, session_id: &str) -> Result<Vec<FileEventDto>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, change_kind, old_path, lines_added, lines_removed, source,
                patch_blob_id IS NOT NULL
         FROM file_event WHERE session_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map([session_id], |row| {
            Ok(FileEventDto {
                id: row.get(0)?,
                path: row.get(1)?,
                change_kind: row.get(2)?,
                old_path: row.get(3)?,
                lines_added: row.get(4)?,
                lines_removed: row.get(5)?,
                source: row.get(6)?,
                has_patch: row.get::<_, i64>(7)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The `(storage_relpath, scan_state)` of a file event's recorded patch blob, or
/// `None` when the event has no stored patch. `scan_state` lets callers enforce
/// the quarantine guarantee: a blob marked `failed_quarantined` was never fully
/// scanned, so its content is unavailable to derived surfaces. The relpath is
/// validated on read.
pub fn file_patch_blob(conn: &Connection, file_event_id: &str) -> Result<Option<(String, String)>> {
    conn.query_row(
        "SELECT b.storage_relpath, b.scan_state
         FROM file_event f JOIN blob b ON b.id = f.patch_blob_id
         WHERE f.id = ?1",
        [file_event_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

/// The recorded patch text for a file event, or `None` when none is stored, the
/// payload is not valid UTF-8, or the blob is quarantined. A `failed_quarantined`
/// blob's scan never completed, so its content is unavailable to derived
/// surfaces including the UI patch read (SECRET_SCANNING.md §6).
pub fn file_patch_text(
    conn: &Connection,
    blobs: &BlobStore,
    file_event_id: &str,
) -> Result<Option<String>> {
    let Some((relpath, scan_state)) = file_patch_blob(conn, file_event_id)? else {
        return Ok(None);
    };
    if scan_state == "failed_quarantined" {
        return Ok(None);
    }
    match blobs.read(&relpath) {
        Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
        Err(e) => Err(e),
    }
}

/// How many secret findings were flagged in a session (all are redacted from
/// derived surfaces; the canonical copy may still contain them).
pub fn secret_count(conn: &Connection, session_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT count(*) FROM secret_finding WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
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
    // `is_missing` is derived, not stored: a repository is missing only when it
    // has at least one worktree and every one of them is missing (F15). The
    // stored `repository.is_missing` column is never written and is ignored
    // here, so the two flags cannot drift.
    let mut stmt = conn.prepare(
        "SELECT r.id, r.display_name, r.identity_confidence, r.primary_path,
                (SELECT CASE
                    WHEN count(*) = 0 THEN 0
                    WHEN count(*) FILTER (WHERE w.is_missing = 1) = count(*) THEN 1
                    ELSE 0 END
                 FROM worktree w WHERE w.repository_id = r.id),
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

    fn codex_patch(session: &str, content: &str) -> String {
        format!(
            concat!(
                "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{{\"id\":\"{id}\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}}}\n",
                "{{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{{\"type\":\"function_call\",\"name\":\"apply_patch\",\"arguments\":\"{{}}\",\"call_id\":\"c1\"}}}}\n",
                "{{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:02.000Z\",\"payload\":{{\"type\":\"patch_apply_end\",\"call_id\":\"c1\",\"success\":true,\"changes\":{{\"config.ts\":{{\"type\":\"add\",\"content\":\"{content}\"}}}}}}}}\n"
            ),
            id = session,
            content = content
        )
    }

    fn secret() -> String {
        format!("ghp{}", "_0123456789abcdefghijklmnopqrstuvwxyz")
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
    fn counts_sessions_for_an_agent() {
        let conn = crate::storage::open_in_memory().unwrap();
        seed(&conn);
        assert_eq!(agent_session_count(&conn, "claude-code").unwrap(), 1);
        assert_eq!(agent_session_count(&conn, "codex").unwrap(), 0);
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
    fn session_cursor_round_trips_timestamp_and_escaped_id() {
        for started_at in [Some(1_723_372_800_000), None, Some(-1)] {
            let cursor = SessionCursor {
                started_at,
                id: "native:id%3Awith-percent".into(),
            };
            assert_eq!(SessionCursor::decode(&cursor.encode()), Some(cursor));
        }
        for malformed in ["", "missing-separator", "n:", "not-a-time:id", "n:id:extra"] {
            assert!(SessionCursor::decode(malformed).is_none());
        }
    }

    #[test]
    fn session_pages_reproduce_the_full_stable_order() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        for i in 0..5 {
            let content = format!(
                "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:0{i}.000Z\",\"payload\":{{\"id\":\"cx{i}\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}}}\n"
            );
            let parsed = CodexAdapter::new().parse_str(&content, &format!("cx{i}"));
            persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();
        }
        // Claude fixture has no timestamp, exercising the trailing NULL block.
        seed(&conn);

        let expected: Vec<String> = list_sessions(&conn, 100)
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert_eq!(expected.len(), 6);

        let mut actual = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = list_sessions_page(&conn, 2, cursor.as_deref()).unwrap();
            actual.extend(page.sessions.into_iter().map(|session| session.id));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(actual, expected);
        let unique: std::collections::HashSet<_> = actual.iter().collect();
        assert_eq!(
            unique.len(),
            actual.len(),
            "no session repeats across pages"
        );

        let first = list_sessions_page(&conn, 2, None).unwrap();
        let malformed = list_sessions_page(&conn, 2, Some("not-a-cursor")).unwrap();
        assert_eq!(malformed, first, "a malformed cursor degrades to page one");
    }

    #[test]
    fn repository_session_pages_stay_scoped_and_stable() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        conn.execute(
            "INSERT INTO repository
                (id, identity_key, display_name, identity_confidence, created_at, updated_at)
             VALUES ('repo-a', 'repo-a', 'Repo A', 'confirmed', 1, 1)",
            [],
        )
        .unwrap();

        for i in 0..5 {
            let content = format!(
                concat!(
                    "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:0{i}.000Z\",\"payload\":{{\"id\":\"repo-cx{i}\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}}}\n",
                    "{{\"type\":\"turn_context\",\"timestamp\":\"2026-08-11T10:00:0{i}.000Z\",\"payload\":{{\"cwd\":\"/p\",\"model\":\"gpt-x\",\"turn_id\":\"t{i}\"}}}}\n",
                    "{{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:0{i}.000Z\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"hello\"}}}}\n"
                ),
                i = i,
            );
            let parsed = CodexAdapter::new().parse_str(&content, &format!("repo-cx{i}"));
            let id = persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();
            if i != 2 {
                conn.execute(
                    "UPDATE session_segment SET repository_id = 'repo-a' WHERE session_id = ?1",
                    [&id],
                )
                .unwrap();
            }
        }

        let expected: Vec<String> = list_repository_sessions(&conn, "repo-a", 100)
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert_eq!(expected.len(), 4, "one unlinked session is excluded");

        let first = list_repository_sessions_page(&conn, "repo-a", 2, None).unwrap();
        assert_eq!(first.sessions.len(), 2);
        let second =
            list_repository_sessions_page(&conn, "repo-a", 2, first.next_cursor.as_deref())
                .unwrap();
        let actual: Vec<String> = first
            .sessions
            .into_iter()
            .chain(second.sessions)
            .map(|session| session.id)
            .collect();
        assert_eq!(actual, expected);
        assert!(second.next_cursor.is_none());
    }

    #[test]
    fn folder_session_pages_stay_scoped_and_stable() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();

        let folder_a = crate::folders::create_folder(&conn, "Folder A").unwrap();
        let folder_b = crate::folders::create_folder(&conn, "Folder B").unwrap();

        let mut folder_a_ids = Vec::new();
        for i in 0..5 {
            let content = format!(
                concat!(
                    "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:0{i}.000Z\",\"payload\":{{\"id\":\"fldr-cx{i}\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}}}\n",
                    "{{\"type\":\"turn_context\",\"timestamp\":\"2026-08-11T10:00:0{i}.000Z\",\"payload\":{{\"cwd\":\"/p\",\"model\":\"gpt-x\",\"turn_id\":\"t{i}\"}}}}\n",
                    "{{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:0{i}.000Z\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"hello\"}}}}\n"
                ),
                i = i,
            );
            let parsed = CodexAdapter::new().parse_str(&content, &format!("fldr-cx{i}"));
            let id = persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();
            if i % 2 == 0 {
                crate::folders::set_session_folder(&conn, &id, Some(&folder_a.id)).unwrap();
                folder_a_ids.push(id);
            } else {
                crate::folders::set_session_folder(&conn, &id, Some(&folder_b.id)).unwrap();
            }
        }
        // Newest first order for Folder A: i=4, 2, 0
        folder_a_ids.reverse();

        // Folder A contains i=4, 2, 0 (3 sessions total, newest first).
        let first = list_folder_sessions_page(&conn, &folder_a.id, 2, None).unwrap();
        assert_eq!(first.sessions.len(), 2);
        assert!(first.next_cursor.is_some());

        let second =
            list_folder_sessions_page(&conn, &folder_a.id, 2, first.next_cursor.as_deref())
                .unwrap();
        assert_eq!(second.sessions.len(), 1);
        assert!(second.next_cursor.is_none());

        let actual: Vec<String> = first
            .sessions
            .into_iter()
            .chain(second.sessions)
            .map(|s| s.id)
            .collect();
        assert_eq!(actual, folder_a_ids);

        // Empty folder produces an empty page with no cursor.
        let empty_folder = crate::folders::create_folder(&conn, "Empty").unwrap();
        let empty_page = list_folder_sessions_page(&conn, &empty_folder.id, 10, None).unwrap();
        assert!(empty_page.sessions.is_empty());
        assert!(empty_page.next_cursor.is_none());
    }

    #[test]
    fn session_pages_handle_identical_timestamps_without_skips_or_duplicates() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();

        // 6 sessions sharing the exact same millisecond timestamp
        for i in 0..6 {
            let content = "{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"id\":\"same-time-";
            let full_content = format!("{content}{i}\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}\n");
            let parsed = CodexAdapter::new().parse_str(&full_content, &format!("same-time-{i}"));
            persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();
        }

        let expected: Vec<String> = list_sessions(&conn, 100)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(expected.len(), 6);

        let mut actual = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = list_sessions_page(&conn, 2, cursor.as_deref()).unwrap();
            actual.extend(page.sessions.into_iter().map(|s| s.id));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }

        assert_eq!(actual, expected);
        let unique: std::collections::HashSet<_> = actual.iter().collect();
        assert_eq!(
            unique.len(),
            6,
            "all rows with identical timestamps paginated without duplicates"
        );
    }

    #[test]
    fn session_cursor_roundtrips_complex_ids() {
        for (started_at, id) in [
            (Some(1_700_000_000_i64), "simple-id"),
            (None, "null-time-id"),
            (Some(123), "id:with:colons"),
            (Some(456), "id%with%percent"),
            (Some(789), "id%3Awith%25encoded"),
            (None, "%253A"),
        ] {
            let encoded = SessionCursor::encode_parts(started_at, id);
            let decoded = SessionCursor::decode(&encoded).expect("cursor decodes");
            assert_eq!(decoded.started_at, started_at);
            assert_eq!(decoded.id, id);
        }

        // Test percent decode fallback for partial or non-escaped '%'
        let decoded_partial = SessionCursor::decode("100:id%2").expect("cursor decodes");
        assert_eq!(decoded_partial.id, "id%2");

        let decoded_trailing = SessionCursor::decode("100:id%").expect("cursor decodes");
        assert_eq!(decoded_trailing.id, "id%");

        let decoded_unknown = SessionCursor::decode("100:id%99").expect("cursor decodes");
        assert_eq!(decoded_unknown.id, "id%99");
    }

    #[test]
    fn session_cursor_rejects_malformed_inputs() {
        for bad in [
            "",
            "nocolon",
            ":empty_time",
            "100:",
            "not_an_int:valid_id",
            "100:id:with:colons",
        ] {
            assert!(
                SessionCursor::decode(bad).is_none(),
                "{bad:?} should be rejected"
            );
        }
        let oversized = "100:".to_string() + &"x".repeat(3_000);
        assert!(
            SessionCursor::decode(&oversized).is_none(),
            "oversized cursor should be rejected"
        );
    }

    #[test]
    fn empty_database_lists_nothing() {
        let conn = crate::storage::open_in_memory().unwrap();
        assert!(list_sessions(&conn, 10).unwrap().is_empty());
        assert!(list_repositories(&conn).unwrap().is_empty());
        let page = list_sessions_page(&conn, 10, None).unwrap();
        assert!(page.sessions.is_empty());
        assert!(page.next_cursor.is_none());
        let repo_page = list_repository_sessions_page(&conn, "nonexistent-repo", 10, None).unwrap();
        assert!(repo_page.sessions.is_empty());
        assert!(repo_page.next_cursor.is_none());
        let folder_page = list_folder_sessions_page(&conn, "nonexistent-folder", 10, None).unwrap();
        assert!(folder_page.sessions.is_empty());
        assert!(folder_page.next_cursor.is_none());
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
    fn get_session_exposes_parse_diagnostics_without_inflating_list_rows() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let content = concat!(
            "{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"id\":\"partial\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{\"type\":\"brand_new_event\"}}\n",
        );
        let parsed = CodexAdapter::new().parse_str(content, "partial");
        let sid = persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();

        let summary_json = serde_json::to_value(&list_sessions(&conn, 1).unwrap()[0]).unwrap();
        assert!(
            summary_json.get("parse_note").is_none(),
            "browse summaries stay sparse"
        );

        let detail_json = serde_json::to_value(get_session(&conn, &sid).unwrap().unwrap()).unwrap();
        assert_eq!(
            detail_json
                .get("parse_note")
                .and_then(|value| value.as_str()),
            Some("1 parser note(s); first: unknown event_msg: brand_new_event")
        );
    }

    #[test]
    fn file_patch_relpath_resolves_a_recorded_patch_blob() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let parsed = CodexAdapter::new().parse_str(&fixture("codex", "patch_apply.jsonl"), "p");
        let sid = persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();

        let detail = get_session(&conn, &sid).unwrap().unwrap();
        let edited = detail
            .file_events
            .iter()
            .find(|f| f.path == "src/edit.ts")
            .expect("edited file present");
        assert!(edited.has_patch);

        let (relpath, scan_state) = file_patch_blob(&conn, &edited.id)
            .unwrap()
            .expect("a patch blob is referenced");
        assert_eq!(scan_state, "clean");
        let bytes = blobs.read(&relpath).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("-old") && text.contains("+new"));
        assert!(file_patch_text(&conn, &blobs, &edited.id)
            .unwrap()
            .is_some());

        // A file event with no recorded patch resolves to None.
        let no_patch = file_patch_blob(&conn, "does-not-exist").unwrap();
        assert!(no_patch.is_none());
        assert!(file_patch_text(&conn, &blobs, "does-not-exist")
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_quarantined_patch_blob_is_unavailable_to_the_ui_read() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();

        // Healthy recorded patch: readable with a clean scan_state.
        let healthy = CodexAdapter::new().parse_str(&fixture("codex", "patch_apply.jsonl"), "p");
        let hsid = persist_session(&conn, "codex", "Codex", &healthy, &blobs).unwrap();
        let edited = get_session(&conn, &hsid)
            .unwrap()
            .unwrap()
            .file_events
            .into_iter()
            .find(|f| f.has_patch)
            .expect("healthy patch present");
        assert!(file_patch_text(&conn, &blobs, &edited.id)
            .unwrap()
            .is_some());

        // A scanner failure quarantines the blob; its content must not reach the
        // UI patch read (SECRET_SCANNING.md §6).
        let content = format!("const KEY = {}", secret());
        let quarantined = CodexAdapter::new().parse_str(&codex_patch("s-q", &content), "s-q");
        crate::secrets::set_fail_scans_for_test(true);
        let qsid = persist_session(&conn, "codex", "Codex", &quarantined, &blobs).unwrap();
        crate::secrets::set_fail_scans_for_test(false);

        let patch_event = get_session(&conn, &qsid)
            .unwrap()
            .unwrap()
            .file_events
            .into_iter()
            .find(|f| f.has_patch)
            .expect("quarantined patch present");
        let (_, scan_state) = file_patch_blob(&conn, &patch_event.id)
            .unwrap()
            .expect("blob exists");
        assert_eq!(scan_state, "failed_quarantined");
        assert!(
            file_patch_text(&conn, &blobs, &patch_event.id)
                .unwrap()
                .is_none(),
            "un-scanned patch content is unavailable to the UI read"
        );
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

    #[test]
    fn list_folder_sessions_page_paginates_and_handles_empty_folders() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();

        let claude =
            ClaudeCodeAdapter::new().parse_str(&fixture("claude_code", "basic_text.jsonl"), "b");
        let s1 = persist_session(&conn, "claude-code", "Claude Code", &claude, &blobs).unwrap();

        let codex = CodexAdapter::new().parse_str(&fixture("codex", "minimal.jsonl"), "codex_b");
        let s2 = persist_session(&conn, "codex", "Codex", &codex, &blobs).unwrap();

        let f1 = crate::folders::create_folder(&conn, "Auth Refactor").unwrap();
        let f_empty = crate::folders::create_folder(&conn, "Empty Folder").unwrap();

        crate::folders::set_session_folder(&conn, &s1, Some(&f1.id)).unwrap();
        crate::folders::set_session_folder(&conn, &s2, Some(&f1.id)).unwrap();

        // Query empty folder -> 0 sessions, no cursor.
        let empty_page = list_folder_sessions_page(&conn, &f_empty.id, 10, None).unwrap();
        assert_eq!(empty_page.sessions.len(), 0);
        assert!(empty_page.next_cursor.is_none());

        // Query non-existent folder -> 0 sessions, no error.
        let missing_page = list_folder_sessions_page(&conn, "nonexistent", 10, None).unwrap();
        assert_eq!(missing_page.sessions.len(), 0);
        assert!(missing_page.next_cursor.is_none());

        // Paged query: limit 1 returns 1 session and next_cursor.
        let page1 = list_folder_sessions_page(&conn, &f1.id, 1, None).unwrap();
        assert_eq!(page1.sessions.len(), 1);
        assert!(page1.next_cursor.is_some());

        // Page 2 using cursor returns remaining 1 session and no further cursor.
        let page2 =
            list_folder_sessions_page(&conn, &f1.id, 1, page1.next_cursor.as_deref()).unwrap();
        assert_eq!(page2.sessions.len(), 1);
        assert!(page2.next_cursor.is_none());
        assert_ne!(page1.sessions[0].id, page2.sessions[0].id);

        // Malformed cursor degrades to first page without error.
        let malformed_page =
            list_folder_sessions_page(&conn, &f1.id, 1, Some("not-valid-cursor")).unwrap();
        assert_eq!(malformed_page.sessions.len(), 1);
        assert_eq!(malformed_page.sessions[0].id, page1.sessions[0].id);

        // Limit <= 0 clamps to 1.
        let clamped_zero = list_folder_sessions_page(&conn, &f1.id, 0, None).unwrap();
        assert_eq!(clamped_zero.sessions.len(), 1);
    }

    #[test]
    fn get_token_totals_aggregates_usage_and_cost() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();

        let initial_totals = get_token_totals(&conn).unwrap();
        assert_eq!(initial_totals.total_input_tokens, 0);
        assert_eq!(initial_totals.total_output_tokens, 0);
        assert_eq!(initial_totals.total_cache_tokens, 0);
        assert_eq!(initial_totals.est_cost_usd, 0.0);

        let codex =
            CodexAdapter::new().parse_str(&fixture("codex", "token_count.jsonl"), "codex_totals");
        let session_id = persist_session(&conn, "codex", "Codex", &codex, &blobs).unwrap();

        let detail = get_session(&conn, &session_id).unwrap().unwrap();
        assert_eq!(detail.summary.total_input_tokens, Some(120));
        assert_eq!(detail.summary.total_output_tokens, Some(30));
        assert_eq!(detail.summary.total_cache_tokens, Some(45));

        let totals = get_token_totals(&conn).unwrap();
        assert_eq!(totals.total_input_tokens, 120);
        assert_eq!(totals.total_output_tokens, 30);
        assert_eq!(totals.total_cache_tokens, 45);
    }
}
