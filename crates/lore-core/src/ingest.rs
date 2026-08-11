//! Ingest: persist a normalized [`ParsedSession`] into the SQLite schema.
//!
//! Persistence is one transaction and is **idempotent**: re-persisting the same
//! logical session replaces its rows (delete-cascade then insert), so re-ingest
//! after an append/rewrite never duplicates. Row ids are deterministic
//! functions of stable natural keys, which is what makes replacement safe.
//! Message `parent_id` is resolved from `parent_native_uuid` against a map built
//! over *all* messages first, so out-of-order parents resolve correctly.
//!
//! Search projections and secret scanning are added in M6; this module writes
//! canonical rows only.

use std::collections::HashMap;

use rusqlite::{params, Connection};

use crate::model::ParsedSession;
use crate::storage::Result;

/// Persist a parsed session, returning its database id. Idempotent.
pub fn persist_session(
    conn: &Connection,
    agent_id: &str,
    agent_name: &str,
    parsed: &ParsedSession,
) -> Result<String> {
    let tx = conn.unchecked_transaction()?;
    // Defer FK checks to commit so forward self-references (a child message whose
    // parent is inserted later) and other out-of-order links are legal mid-txn.
    // Resets automatically after commit.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON;")?;

    tx.execute(
        "INSERT INTO agent (id, display_name, detected) VALUES (?1, ?2, 1)
         ON CONFLICT(id) DO UPDATE SET display_name = excluded.display_name",
        params![agent_id, agent_name],
    )?;

    let natural_key = parsed
        .native_session_id
        .as_deref()
        .unwrap_or(&parsed.dedupe_key);
    let session_id = det_id("s", &[agent_id, natural_key]);

    // Idempotent replace: cascade-delete any prior rows for this session.
    tx.execute(
        "DELETE FROM agent_session WHERE id = ?1",
        params![session_id],
    )?;

    tx.execute(
        "INSERT INTO agent_session
            (id, agent_id, native_session_id, dedupe_key, title, started_at, ended_at,
             primary_model, message_count, tool_call_count, parse_status, parse_note, agent_version)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![
            session_id,
            agent_id,
            parsed.native_session_id,
            parsed.dedupe_key,
            parsed.title,
            parsed.started_at,
            parsed.ended_at,
            parsed.primary_model,
            parsed.messages.len() as i64,
            parsed.tool_calls.len() as i64,
            parsed.status.as_str(),
            parse_note(parsed),
            parsed.agent_version,
        ],
    )?;

    // Segments.
    let mut segment_ids = Vec::with_capacity(parsed.segments.len());
    for (i, seg) in parsed.segments.iter().enumerate() {
        let sid = det_id("seg", &[&session_id, &i.to_string()]);
        tx.execute(
            "INSERT INTO session_segment
                (id, session_id, seq_start, seq_end, cwd, model, provider,
                 context_source, resolution_confidence)
             VALUES (?1,?2,?3,?4,?5,?6,?7,'event','unresolved')",
            params![
                sid,
                session_id,
                seg.seq_start,
                seg.seq_end,
                seg.cwd,
                seg.model,
                seg.provider
            ],
        )?;
        segment_ids.push(sid);
    }

    // Pre-assign message ids and build the uuid->id map for parent resolution.
    let mut msg_ids = Vec::with_capacity(parsed.messages.len());
    let mut uuid_to_id: HashMap<String, String> = HashMap::new();
    for m in &parsed.messages {
        let mid = det_id("m", &[&session_id, &m.seq.to_string()]);
        if let Some(u) = &m.native_uuid {
            uuid_to_id.insert(u.clone(), mid.clone());
        }
        msg_ids.push(mid);
    }

    // Messages + parts.
    let mut part_ids: HashMap<(i64, i64), String> = HashMap::new();
    for (mi, m) in parsed.messages.iter().enumerate() {
        let mid = &msg_ids[mi];
        let segment_id = segment_ids.get(m.segment_ix);
        let parent_id = m
            .parent_native_uuid
            .as_ref()
            .and_then(|u| uuid_to_id.get(u))
            .cloned();
        tx.execute(
            "INSERT INTO message
                (id, session_id, segment_id, native_uuid, parent_id, parent_native_uuid, seq,
                 role, event_kind, is_sidechain, ts, model, token_input, token_output,
                 token_cache, stop_reason, source_offset)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                mid,
                session_id,
                segment_id,
                m.native_uuid,
                parent_id,
                m.parent_native_uuid,
                m.seq,
                m.role.as_str(),
                m.event_kind.as_str(),
                m.is_sidechain,
                m.ts,
                m.model,
                m.tokens.input,
                m.tokens.output,
                m.tokens.cache,
                m.stop_reason,
                m.source_offset,
            ],
        )?;
        for p in &m.parts {
            let pid = det_id("p", &[mid, &p.ordinal.to_string()]);
            tx.execute(
                "INSERT INTO message_part
                    (id, message_id, ordinal, kind, text, content_json, searchable, metadata_json)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    pid,
                    mid,
                    p.ordinal,
                    p.kind.as_str(),
                    p.text,
                    p.content_json,
                    p.searchable,
                    p.metadata_json,
                ],
            )?;
            part_ids.insert((m.seq, p.ordinal), pid);
        }
    }

    // Tool calls (the invocation part must exist to satisfy the NOT NULL FK).
    for tc in &parsed.tool_calls {
        let Some(call_part_id) = part_ids.get(&tc.call_ref) else {
            continue;
        };
        let result_part_id = tc.result_ref.as_ref().and_then(|r| part_ids.get(r));
        let tid = det_id("t", &[&session_id, &tc.native_call_id]);
        tx.execute(
            "INSERT INTO tool_call
                (id, session_id, call_part_id, result_part_id, native_call_id, name,
                 input_json, output_text, is_error, duration_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                tid,
                session_id,
                call_part_id,
                result_part_id,
                tc.native_call_id,
                tc.name,
                tc.input_json,
                tc.output_text,
                tc.is_error,
                tc.duration_ms,
            ],
        )?;
    }

    // File events.
    for (i, fe) in parsed.file_events.iter().enumerate() {
        let fid = det_id("f", &[&session_id, &i.to_string()]);
        let segment_id = segment_ids.get(fe.segment_ix);
        let tool_call_id = fe
            .tool_native_call_id
            .as_ref()
            .map(|c| det_id("t", &[&session_id, c]));
        tx.execute(
            "INSERT INTO file_event
                (id, session_id, segment_id, tool_call_id, path, change_kind, old_path,
                 lines_added, lines_removed, source, event_ts)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                fid,
                session_id,
                segment_id,
                tool_call_id,
                fe.path,
                fe.change_kind.as_str(),
                fe.old_path,
                fe.lines_added,
                fe.lines_removed,
                fe.source.as_str(),
                fe.event_ts,
            ],
        )?;
    }

    // Agent-recorded git observations from segment context (branch/commit as the
    // agent stamped them; never labeled a captured session-time snapshot).
    for (i, seg) in parsed.segments.iter().enumerate() {
        if seg.git_branch.is_none() && seg.git_commit_sha.is_none() {
            continue;
        }
        let gid = det_id("g", &[&session_id, &i.to_string()]);
        tx.execute(
            "INSERT INTO git_observation
                (id, session_id, segment_id, source, observed_at, temporal_confidence,
                 branch, commit_sha, remote_url_norm)
             VALUES (?1,?2,?3,'agent_recorded', unixepoch('now')*1000, 'near_event', ?4,?5,?6)",
            params![
                gid,
                session_id,
                segment_ids[i],
                seg.git_branch,
                seg.git_commit_sha,
                seg.git_remote_url,
            ],
        )?;
    }

    tx.commit()?;
    Ok(session_id)
}

/// A bounded, content-free note summarizing parser diagnostics.
fn parse_note(parsed: &ParsedSession) -> Option<String> {
    if let Some(n) = &parsed.note {
        return Some(bounded(n));
    }
    match parsed.notes.len() {
        0 => None,
        n => Some(bounded(&format!(
            "{n} parser note(s); first: {}",
            parsed.notes[0].message
        ))),
    }
}

fn bounded(s: &str) -> String {
    s.chars().take(500).collect()
}

/// Deterministic opaque id from a prefix and stable natural-key parts.
fn det_id(prefix: &str, parts: &[&str]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            hash ^= 0x1f;
            hash = hash.wrapping_mul(PRIME);
        }
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    format!("{prefix}_{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn det_id_is_deterministic_and_distinct() {
        assert_eq!(det_id("m", &["s", "0"]), det_id("m", &["s", "0"]));
        assert_ne!(det_id("m", &["s", "0"]), det_id("m", &["s", "1"]));
        // Boundary between parts matters ("a","b" != "ab","").
        assert_ne!(det_id("x", &["a", "b"]), det_id("x", &["ab", ""]));
    }
}
