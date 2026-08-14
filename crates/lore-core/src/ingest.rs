//! Ingest: persist a normalized [`ParsedSession`] into the SQLite schema.
//!
//! Persistence is one transaction and is **idempotent**: re-persisting the same
//! logical session replaces its rows (delete-cascade then insert), so re-ingest
//! after an append/rewrite never duplicates. Row ids are deterministic
//! functions of stable natural keys, which is what makes replacement safe.
//! Message `parent_id` is resolved from `parent_native_uuid` against a map built
//! over *all* messages first, so out-of-order parents resolve correctly.
//!
//! Every cleartext field is secret-scanned before it can be indexed or
//! exported: findings are recorded and searchable fields get a **redacted**
//! `SearchDocument` projection, so a flagged span never reaches FTS
//! (`SECRET_SCANNING.md`, `SEARCH.md`). Recorded-patch blobs finalize their
//! `scan_state` here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::adapters::{AdapterRegistry, AgentAdapter, SessionRef};
use crate::model::ParsedSession;
use crate::storage::blob::{BlobStore, StagedBlob};
use crate::storage::{Result, StorageError};

/// Bumped when parser output for a source could change; drives re-ingest.
const PARSER_VERSION: &str = "2";
/// Bytes of a source file hashed to distinguish append from rewrite.
const PREFIX_BYTES: usize = 4096;
/// Media type recorded for agent-recorded patch/diff blobs.
const PATCH_MEDIA_TYPE: &str = "text/x-patch";

/// Persist a parsed session in its own transaction. Idempotent.
pub fn persist_session(
    conn: &Connection,
    agent_id: &str,
    agent_name: &str,
    parsed: &ParsedSession,
    blobs: &BlobStore,
) -> Result<String> {
    // Blob files are content-addressed and finalized on disk before the write
    // transaction opens (DATA_MODEL.md §3); the row that references them commits
    // atomically with the normalized rows below.
    let patch_blobs = stage_patch_blobs(blobs, parsed)?;
    let tx = conn.unchecked_transaction()?;
    let session_id = persist_rows(&tx, agent_id, agent_name, parsed, None, &patch_blobs)?;
    tx.commit()?;
    Ok(session_id)
}

/// Durably stage the recorded patch payload for every file event that carries
/// one, keeping the result index-aligned with `parsed.file_events`. Staging is
/// done before any transaction opens so the write lock is not held during file
/// I/O.
fn stage_patch_blobs(blobs: &BlobStore, parsed: &ParsedSession) -> Result<Vec<Option<StagedBlob>>> {
    parsed
        .file_events
        .iter()
        .map(|fe| match &fe.patch_text {
            Some(text) => blobs.stage(text.as_bytes()).map(Some),
            None => Ok(None),
        })
        .collect()
}

/// Write all normalized rows for a session into the caller-owned transaction
/// `tx`. Separating this from transaction management lets the ingest service
/// bundle source-artifact tracking and the checkpoint atomically with the rows.
/// `source_artifact_id` stamps message provenance when known.
fn persist_rows(
    tx: &Connection,
    agent_id: &str,
    agent_name: &str,
    parsed: &ParsedSession,
    source_artifact_id: Option<&str>,
    patch_blobs: &[Option<StagedBlob>],
) -> Result<String> {
    // Defer FK checks to commit so forward self-references (a child message whose
    // parent is inserted later) and other out-of-order links are legal mid-txn.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON;")?;

    tx.execute(
        "INSERT INTO agent (id, display_name, detected) VALUES (?1, ?2, 1)
         ON CONFLICT(id) DO UPDATE SET display_name = excluded.display_name",
        params![agent_id, agent_name],
    )?;

    let session_id = session_id_for(agent_id, parsed);
    let existing_sources = session_sources(tx, &session_id)?;

    // Explicitly delete search projections first so the FTS external-content
    // delete trigger fires (an FK cascade would not fire it), then cascade-delete
    // the rest of the session's rows. Idempotent replace.
    tx.execute(
        "DELETE FROM search_document WHERE session_id = ?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM agent_session WHERE id = ?1",
        params![session_id],
    )?;

    // Deleting the logical session above cascades its source links. Preserve
    // links from copies/continued artifacts so replaying one source does not
    // orphan the others.
    for source in existing_sources {
        tx.execute(
            "INSERT INTO session_source (session_id, source_artifact_id, first_seq, last_seq)
             VALUES (?1,?2,?3,?4)",
            params![
                session_id,
                source.artifact_id,
                source.first_seq,
                source.last_seq
            ],
        )?;
    }

    let total_tokens = session_token_totals(parsed);
    tx.execute(
        "INSERT INTO agent_session
            (id, agent_id, native_session_id, dedupe_key, title, started_at, ended_at,
             primary_model, message_count, tool_call_count, total_input_tokens,
             total_output_tokens, total_cache_tokens, parse_status, parse_note, agent_version)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
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
            total_tokens.input,
            total_tokens.output,
            total_tokens.cache,
            parsed.status.as_str(),
            parse_note(parsed),
            parsed.agent_version,
        ],
    )?;

    // Title is a searchable projection (scanned + redacted like any cleartext).
    if let Some(title) = &parsed.title {
        scan_and_project(
            tx,
            &session_id,
            None,
            "session",
            &session_id,
            "title",
            title,
            true,
        )?;
    }

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
                 token_cache, stop_reason, source_artifact_id, source_offset)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
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
                source_artifact_id,
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

            // Scan every cleartext part for secrets. Index the text projection
            // only when the part is searchable (thinking is scanned, not
            // indexed); tool payload JSON is scanned but never indexed.
            let seg = segment_id.map(String::as_str);
            if let Some(text) = &p.text {
                scan_and_project(
                    tx,
                    &session_id,
                    seg,
                    "message_part",
                    &pid,
                    "text",
                    text,
                    p.searchable,
                )?;
            }
            if let Some(content) = &p.content_json {
                scan_and_project(
                    tx,
                    &session_id,
                    seg,
                    "message_part",
                    &pid,
                    "content_json",
                    content,
                    false,
                )?;
            }
            part_ids.insert((m.seq, p.ordinal), pid);
        }
    }

    // Tool calls (the invocation part must exist to satisfy the NOT NULL FK).
    let mut persisted_tool_call_ids = HashSet::new();
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
        persisted_tool_call_ids.insert(tc.native_call_id.as_str());
    }

    // File events. A recorded patch payload is referenced from its staged blob
    // (byte-faithful; the diff itself lives in the content-addressed store).
    for (i, fe) in parsed.file_events.iter().enumerate() {
        let fid = det_id("f", &[&session_id, &i.to_string()]);
        let segment_id = segment_ids.get(fe.segment_ix);
        let tool_call_id = fe
            .tool_native_call_id
            .as_ref()
            .filter(|call_id| persisted_tool_call_ids.contains(call_id.as_str()))
            .map(|c| det_id("t", &[&session_id, c]));
        let patch_blob_id = match patch_blobs.get(i).and_then(Option::as_ref) {
            Some(staged) => Some(BlobStore::reference(tx, staged, PATCH_MEDIA_TYPE)?),
            None => None,
        };
        tx.execute(
            "INSERT INTO file_event
                (id, session_id, segment_id, tool_call_id, path, change_kind, old_path,
                 lines_added, lines_removed, patch_blob_id, source, event_ts)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
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
                patch_blob_id,
                fe.source.as_str(),
                fe.event_ts,
            ],
        )?;

        // Scan the recorded patch, index a redacted projection, and finalize the
        // blob's scan_state so it becomes available to search/export. A scanner
        // failure quarantines the patch: no projection, blob marked
        // `failed_quarantined` (SECRET_SCANNING.md §6).
        if let Some(patch) = &fe.patch_text {
            let seg = segment_id.map(String::as_str);
            let outcome = scan_and_project(
                tx,
                &session_id,
                seg,
                "file_event",
                &fid,
                "patch",
                patch,
                true,
            )?;
            if let Some(blob_id) = &patch_blob_id {
                let state = match outcome {
                    ScanOutcome::Failed => "failed_quarantined",
                    ScanOutcome::Findings => "findings",
                    ScanOutcome::Clean => "clean",
                };
                tx.execute(
                    "UPDATE blob SET scan_state = ?2 WHERE id = ?1",
                    params![blob_id, state],
                )?;
            }
        }
    }

    // Agent-recorded git observations from segment context (branch/commit as the
    // agent stamped them; never labeled a captured session-time snapshot).
    for (i, seg) in parsed.segments.iter().enumerate() {
        if seg.git_branch.is_none() && seg.git_commit_sha.is_none() {
            continue;
        }
        let gid = det_id("g", &[&session_id, &i.to_string()]);
        // Store only the credential-free normalized remote; a recorded URL may
        // carry an embedded token and must never be persisted verbatim.
        let remote_url_norm = seg
            .git_remote_url
            .as_deref()
            .and_then(crate::git::normalize_remote_url);
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
                remote_url_norm,
            ],
        )?;
    }

    Ok(session_id)
}

fn session_token_totals(parsed: &ParsedSession) -> crate::model::Tokens {
    crate::model::Tokens {
        input: parsed
            .total_tokens
            .input
            .or_else(|| sum_message_tokens(parsed, |tokens| tokens.input)),
        output: parsed
            .total_tokens
            .output
            .or_else(|| sum_message_tokens(parsed, |tokens| tokens.output)),
        cache: parsed
            .total_tokens
            .cache
            .or_else(|| sum_message_tokens(parsed, |tokens| tokens.cache)),
    }
}

fn sum_message_tokens(
    parsed: &ParsedSession,
    field: impl Fn(crate::model::Tokens) -> Option<i64>,
) -> Option<i64> {
    let mut seen = false;
    let total = parsed.messages.iter().fold(0_i64, |sum, message| {
        let value = field(message.tokens);
        if value.is_some() {
            seen = true;
        }
        sum.saturating_add(value.unwrap_or(0))
    });
    seen.then_some(total)
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

/// What a secret scan of one cleartext field concluded. A `Failed` scan
/// quarantines the content (SECRET_SCANNING.md §6): no findings are recorded,
/// no projection is indexed, and a recorded-patch blob is marked
/// `failed_quarantined` — the field is unavailable to search/export but the
/// rest of the session persists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanOutcome {
    Clean,
    Findings,
    Failed,
}

/// Scan one cleartext field for secrets before it can be indexed or exported.
/// Records a `secret_finding` per detection (offsets + rule + severity + a keyed
/// fingerprint — never a second cleartext copy) and, when `index` is set, writes
/// a **redacted** `SearchDocument` projection so a flagged span never reaches
/// FTS. On a scanner failure the field is quarantined and [`ScanOutcome::Failed`]
/// is returned; the session is not failed.
#[allow(clippy::too_many_arguments)] // a projection target is naturally several fields
fn scan_and_project(
    tx: &Connection,
    session_id: &str,
    segment_id: Option<&str>,
    source_kind: &str,
    source_id: &str,
    field: &str,
    text: &str,
    index: bool,
) -> Result<ScanOutcome> {
    let findings = match crate::secrets::scan(text) {
        Ok(findings) => findings,
        Err(_) => return Ok(ScanOutcome::Failed),
    };
    for (i, finding) in findings.iter().enumerate() {
        let fid = det_id("sf", &[source_id, field, &i.to_string()]);
        tx.execute(
            "INSERT INTO secret_finding
                (id, session_id, source_kind, source_id, field, rule, span_start, span_end,
                 severity, value_fingerprint, disposition, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'redacted', unixepoch('now')*1000)",
            params![
                fid,
                session_id,
                source_kind,
                source_id,
                field,
                finding.rule,
                i64::try_from(finding.start).unwrap_or(i64::MAX),
                i64::try_from(finding.end).unwrap_or(i64::MAX),
                finding.severity.as_str(),
                finding.fingerprint(text),
            ],
        )?;
    }
    if index {
        let redacted = crate::secrets::redact(text, &findings);
        tx.execute(
            "INSERT INTO search_document
                (session_id, segment_id, source_kind, source_id, field, ordinal, redacted_text,
                 created_at)
             VALUES (?1,?2,?3,?4,?5,0,?6, unixepoch('now')*1000)",
            params![
                session_id,
                segment_id,
                source_kind,
                source_id,
                field,
                redacted
            ],
        )?;
    }
    Ok(if findings.is_empty() {
        ScanOutcome::Clean
    } else {
        ScanOutcome::Findings
    })
}

/// Deterministic opaque id from a prefix and stable natural-key parts.
pub(crate) fn det_id(prefix: &str, parts: &[&str]) -> String {
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

// ── Recoverable source-file ingest ──────────────────────────────────────────

/// How a source file changed since the last successful ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeClass {
    New,
    Unchanged,
    Appended,
    Rewritten,
    Truncated,
}

/// Outcome of ingesting one source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    /// Content hash unchanged; nothing re-parsed.
    Skipped,
    Ingested {
        session_id: String,
        source_artifact_id: String,
        generation: i64,
        change: ChangeClass,
    },
}

/// Content-free classification for a source that could not be ingested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestFailureKind {
    AdapterNotRegistered,
    SourceFailed,
}

/// One failed source. The local path is retained for an actionable UI; no
/// source content or parser/SQLite detail is copied into the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestFailure {
    pub agent_id: &'static str,
    pub path: PathBuf,
    pub kind: IngestFailureKind,
}

/// Result of one sequential discovery-to-ingest pass. M3 replaces the
/// sequential loop with a bounded durable job queue while preserving this
/// per-source failure isolation contract.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestReport {
    pub ingested: Vec<IngestOutcome>,
    pub skipped: usize,
    pub failures: Vec<IngestFailure>,
}

impl IngestReport {
    #[must_use]
    pub fn processed(&self) -> usize {
        self.ingested.len() + self.skipped + self.failures.len()
    }
}

/// Route discovered sessions to their registered adapters and ingest each in
/// its own transaction. One unreadable or malformed source does not stop peers.
#[must_use]
pub fn ingest_discovered(
    conn: &Connection,
    registry: &AdapterRegistry,
    sessions: &[SessionRef],
    blobs: &BlobStore,
) -> IngestReport {
    let mut report = IngestReport::default();
    for source in sessions {
        let Some(adapter) = registry.get(source.agent.0) else {
            report.failures.push(IngestFailure {
                agent_id: source.agent.0,
                path: source.path.clone(),
                kind: IngestFailureKind::AdapterNotRegistered,
            });
            continue;
        };
        match ingest_file(conn, adapter, &source.path, blobs) {
            Ok(IngestOutcome::Skipped) => report.skipped += 1,
            Ok(outcome) => report.ingested.push(outcome),
            Err(_) => report.failures.push(IngestFailure {
                agent_id: source.agent.0,
                path: source.path.clone(),
                kind: IngestFailureKind::SourceFailed,
            }),
        }
    }
    report
}

struct SourceRow {
    id: String,
    size: i64,
    full_hash: Option<String>,
    prefix_hash: Option<String>,
    generation: i64,
    parser_version: Option<String>,
    ingest_state: Option<String>,
}

struct SourceSnapshot {
    path: String,
    native_file_id: Option<String>,
    size: i64,
    mtime: Option<i64>,
    full_hash: String,
    prefix_hash: String,
    last_offset: i64,
    last_line: i64,
}

struct SessionSourceRow {
    artifact_id: String,
    first_seq: Option<i64>,
    last_seq: Option<i64>,
}

/// Discover, classify, parse, and persist one source file recoverably.
///
/// Idempotent by construction: a changed file is fully re-parsed and its session
/// rows are replaced, so append / truncate / rewrite all converge to the correct
/// state. Classification (via size + a prefix hash) drives generation bumps and
/// lets an unchanged file be skipped without re-parsing. Source-artifact
/// tracking, the session rows, the session↔source link, and the checkpoint all
/// commit in one transaction.
pub fn ingest_file(
    conn: &Connection,
    adapter: &dyn AgentAdapter,
    path: &Path,
    blobs: &BlobStore,
) -> Result<IngestOutcome> {
    let meta = std::fs::metadata(path).map_err(|_| StorageError::Io)?;
    let content = std::fs::read_to_string(path).map_err(|_| StorageError::Io)?;
    let snapshot = source_snapshot(path, &meta, &content);
    let agent_id = adapter.id().0;

    let prev = find_source(
        conn,
        agent_id,
        snapshot.native_file_id.as_deref(),
        &snapshot.path,
        &snapshot.full_hash,
    )?;

    let (artifact_id, generation, change) = match &prev {
        None => (
            det_id(
                "sa",
                &[
                    agent_id,
                    snapshot.native_file_id.as_deref().unwrap_or(&snapshot.path),
                    &snapshot.path,
                ],
            ),
            0,
            ChangeClass::New,
        ),
        Some(row)
            if row.full_hash.as_deref() == Some(snapshot.full_hash.as_str())
                && row.parser_version.as_deref() == Some(PARSER_VERSION)
                && row.ingest_state.as_deref() == Some("ok") =>
        {
            // Unchanged content: refresh stat/alias only, no re-parse.
            let tx = conn.unchecked_transaction()?;
            update_source(&tx, &row.id, &snapshot, row.generation)?;
            add_path_alias(&tx, &row.id, &snapshot.path)?;
            tx.commit()?;
            return Ok(IngestOutcome::Skipped);
        }
        Some(row) => {
            let same_content = row.full_hash.as_deref() == Some(snapshot.full_hash.as_str());
            let appended = !same_content && is_append(row, content.as_bytes());
            let change = if same_content {
                ChangeClass::Unchanged
            } else if appended {
                ChangeClass::Appended
            } else if snapshot.size < row.size {
                ChangeClass::Truncated
            } else {
                ChangeClass::Rewritten
            };
            let generation = if same_content || appended {
                row.generation
            } else {
                row.generation + 1
            };
            (row.id.clone(), generation, change)
        }
    };

    // Parse the bytes already read above; do not re-read the file from disk.
    let fallback = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| snapshot.path.clone());
    let parsed = adapter.parse_content(&content, &fallback);

    // Stage blob files before opening the transaction (DATA_MODEL.md §3).
    let patch_blobs = stage_patch_blobs(blobs, &parsed)?;

    let tx = conn.unchecked_transaction()?;
    let target_session_id = session_id_for(agent_id, &parsed);
    if prev.is_some() {
        detach_reassigned_source(&tx, &artifact_id, &target_session_id)?;
    }
    let session_id = persist_rows(
        &tx,
        agent_id,
        adapter.metadata().display_name,
        &parsed,
        Some(&artifact_id),
        &patch_blobs,
    )?;

    if prev.is_none() {
        insert_source(&tx, &artifact_id, agent_id, &snapshot, generation)?;
    } else {
        update_source(&tx, &artifact_id, &snapshot, generation)?;
    }
    add_path_alias(&tx, &artifact_id, &snapshot.path)?;

    tx.execute(
        "INSERT INTO session_source (session_id, source_artifact_id, first_seq, last_seq)
         VALUES (?1,?2,?3,?4)
         ON CONFLICT(session_id, source_artifact_id) DO UPDATE SET
            first_seq = excluded.first_seq, last_seq = excluded.last_seq",
        params![
            session_id,
            artifact_id,
            parsed.messages.first().map(|m| m.seq),
            parsed.messages.last().map(|m| m.seq),
        ],
    )?;
    tx.execute(
        "INSERT INTO ingest_state
            (source_artifact_id, generation, last_offset, last_line, prefix_hash,
             parser_version, state, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,'ok', unixepoch('now')*1000)
         ON CONFLICT(source_artifact_id) DO UPDATE SET
            generation = excluded.generation, last_offset = excluded.last_offset,
            last_line = excluded.last_line, prefix_hash = excluded.prefix_hash,
            parser_version = excluded.parser_version, state = 'ok',
            updated_at = excluded.updated_at",
        params![
            artifact_id,
            generation,
            snapshot.last_offset,
            snapshot.last_line,
            snapshot.prefix_hash,
            PARSER_VERSION,
        ],
    )?;
    tx.commit()?;

    Ok(IngestOutcome::Ingested {
        session_id,
        source_artifact_id: artifact_id,
        generation,
        change,
    })
}

fn session_id_for(agent_id: &str, parsed: &ParsedSession) -> String {
    let natural_key = parsed
        .native_session_id
        .as_deref()
        .unwrap_or(&parsed.dedupe_key);
    det_id("s", &[agent_id, natural_key])
}

fn session_sources(tx: &Connection, session_id: &str) -> Result<Vec<SessionSourceRow>> {
    let mut statement = tx.prepare(
        "SELECT source_artifact_id, first_seq, last_seq
         FROM session_source WHERE session_id = ?1",
    )?;
    let rows = statement
        .query_map([session_id], |row| {
            Ok(SessionSourceRow {
                artifact_id: row.get(0)?,
                first_seq: row.get(1)?,
                last_seq: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// If a physical source is rewritten into a different native session, detach
/// it from the old logical session and remove that session only when no other
/// source still supports it.
fn detach_reassigned_source(
    tx: &Connection,
    source_artifact_id: &str,
    target_session_id: &str,
) -> Result<()> {
    let old_session_ids = {
        let mut statement = tx.prepare(
            "SELECT session_id FROM session_source
             WHERE source_artifact_id = ?1 AND session_id <> ?2",
        )?;
        let rows = statement
            .query_map(params![source_artifact_id, target_session_id], |row| {
                row.get(0)
            })?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        rows
    };

    for old_session_id in old_session_ids {
        tx.execute(
            "DELETE FROM session_source WHERE session_id = ?1 AND source_artifact_id = ?2",
            params![old_session_id, source_artifact_id],
        )?;
        let remaining: i64 = tx.query_row(
            "SELECT count(*) FROM session_source WHERE session_id = ?1",
            [&old_session_id],
            |row| row.get(0),
        )?;
        if remaining == 0 {
            tx.execute("DELETE FROM agent_session WHERE id = ?1", [&old_session_id])?;
        }
    }
    Ok(())
}

fn find_source(
    conn: &Connection,
    agent_id: &str,
    native_file_id: Option<&str>,
    path: &str,
    full_hash: &str,
) -> Result<Option<SourceRow>> {
    let map = |r: &rusqlite::Row<'_>| {
        Ok(SourceRow {
            id: r.get(0)?,
            size: r.get(1)?,
            full_hash: r.get(2)?,
            prefix_hash: r.get(3)?,
            generation: r.get(4)?,
            parser_version: r.get(5)?,
            ingest_state: r.get(6)?,
        })
    };
    let by_path = conn
        .query_row(
            "SELECT sa.id, sa.size, sa.full_hash, sa.prefix_hash, sa.generation,
                    ist.parser_version, ist.state
             FROM source_artifact sa
             LEFT JOIN ingest_state ist ON ist.source_artifact_id = sa.id
             WHERE sa.agent_id = ?1 AND sa.current_path = ?2",
            params![agent_id, path],
            map,
        )
        .optional()?;
    if by_path.is_some() {
        return Ok(by_path);
    }

    let Some(fid) = native_file_id else {
        return Ok(None);
    };
    conn.query_row(
        "SELECT sa.id, sa.size, sa.full_hash, sa.prefix_hash, sa.generation,
                ist.parser_version, ist.state
         FROM source_artifact sa
         LEFT JOIN ingest_state ist ON ist.source_artifact_id = sa.id
         WHERE sa.agent_id = ?1 AND sa.native_file_id = ?2 AND sa.full_hash = ?3
         ORDER BY sa.last_seen_at DESC LIMIT 1",
        params![agent_id, fid, full_hash],
        map,
    )
    .optional()
    .map_err(StorageError::from)
}

fn insert_source(
    tx: &Connection,
    id: &str,
    agent_id: &str,
    snapshot: &SourceSnapshot,
    generation: i64,
) -> Result<()> {
    tx.execute(
        "INSERT INTO source_artifact
            (id, agent_id, current_path, native_file_id, size, mtime, full_hash,
             prefix_hash, generation, state, first_seen_at, last_seen_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'active',
                 unixepoch('now')*1000, unixepoch('now')*1000)",
        params![
            id,
            agent_id,
            snapshot.path,
            snapshot.native_file_id,
            snapshot.size,
            snapshot.mtime,
            snapshot.full_hash,
            snapshot.prefix_hash,
            generation
        ],
    )?;
    Ok(())
}

fn update_source(
    tx: &Connection,
    id: &str,
    snapshot: &SourceSnapshot,
    generation: i64,
) -> Result<()> {
    tx.execute(
        "UPDATE source_artifact SET
            current_path = ?2, native_file_id = ?3, size = ?4, mtime = ?5,
            full_hash = ?6, prefix_hash = ?7, generation = ?8, state = 'active',
            last_seen_at = unixepoch('now')*1000
         WHERE id = ?1",
        params![
            id,
            snapshot.path,
            snapshot.native_file_id,
            snapshot.size,
            snapshot.mtime,
            snapshot.full_hash,
            snapshot.prefix_hash,
            generation
        ],
    )?;
    Ok(())
}

fn add_path_alias(tx: &Connection, source_artifact_id: &str, path: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO source_artifact_path (source_artifact_id, path, first_seen_at, last_seen_at)
         VALUES (?1, ?2, unixepoch('now')*1000, unixepoch('now')*1000)
         ON CONFLICT(source_artifact_id, path) DO UPDATE SET last_seen_at = unixepoch('now')*1000",
        params![source_artifact_id, path],
    )?;
    Ok(())
}

fn source_snapshot(path: &Path, meta: &std::fs::Metadata, content: &str) -> SourceSnapshot {
    let bytes = content.as_bytes();
    let (full_hash, prefix_hash, last_offset, last_line) = fingerprint_and_checkpoint(bytes);
    SourceSnapshot {
        path: path.to_string_lossy().into_owned(),
        native_file_id: file_id(meta),
        size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
        mtime: mtime_ms(meta),
        full_hash,
        prefix_hash,
        last_offset,
        last_line,
    }
}

/// Compute both source fingerprints and the complete-record checkpoint in one
/// traversal. Changed multi-megabyte sessions are commonly append-only; avoid
/// walking their full history separately for hashing and newline accounting.
fn fingerprint_and_checkpoint(bytes: &[u8]) -> (String, String, i64, i64) {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut full = OFFSET;
    let mut prefix = OFFSET;
    let mut last_offset = 0usize;
    let mut complete_lines = 0usize;

    for (index, byte) in bytes.iter().copied().enumerate() {
        full ^= u64::from(byte);
        full = full.wrapping_mul(PRIME);
        if index < PREFIX_BYTES {
            prefix ^= u64::from(byte);
            prefix = prefix.wrapping_mul(PRIME);
        }
        if byte == b'\n' {
            last_offset = index.saturating_add(1);
            complete_lines = complete_lines.saturating_add(1);
        }
    }

    (
        format!("{full:016x}"),
        format!("{prefix:016x}"),
        i64::try_from(last_offset).unwrap_or(i64::MAX),
        i64::try_from(complete_lines).unwrap_or(i64::MAX),
    )
}

fn is_append(previous: &SourceRow, current: &[u8]) -> bool {
    let Ok(previous_size) = usize::try_from(previous.size) else {
        return false;
    };
    if current.len() <= previous_size {
        return false;
    }
    let prefix_len = previous_size.min(PREFIX_BYTES);
    previous.prefix_hash.as_deref() == Some(fnv1a_hex(&current[..prefix_len]).as_str())
}

fn mtime_ms(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
}

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("{}:{}", meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_meta: &std::fs::Metadata) -> Option<String> {
    None
}

/// FNV-1a 64-bit hex digest (content fingerprint; not a security hash).
fn fnv1a_hex(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_fingerprint_preserves_hashes_and_complete_line_checkpoint() {
        let bytes = b"first\nsecond\npartial";
        let (full, prefix, offset, lines) = fingerprint_and_checkpoint(bytes);
        assert_eq!(full, fnv1a_hex(bytes));
        assert_eq!(prefix, fnv1a_hex(&bytes[..bytes.len().min(PREFIX_BYTES)]));
        assert_eq!((offset, lines), (13, 2));
    }
    use crate::adapters::codex::CodexAdapter;
    use crate::storage::blob::BlobStore;

    #[test]
    fn det_id_is_deterministic_and_distinct() {
        assert_eq!(det_id("m", &["s", "0"]), det_id("m", &["s", "0"]));
        assert_ne!(det_id("m", &["s", "0"]), det_id("m", &["s", "1"]));
        // Boundary between parts matters ("a","b" != "ab","").
        assert_ne!(det_id("x", &["a", "b"]), det_id("x", &["ab", ""]));
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

    #[test]
    fn a_scan_failure_quarantines_blobs_from_search_and_export() {
        let conn = crate::storage::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();

        let content = format!("const KEY = {}", secret());
        let parsed = CodexAdapter::new().parse_str(&codex_patch("s-a", &content), "s-a");

        // SECRET_SCANNING.md §6: a scanner failure becomes `failed_quarantined`
        // and the content is unavailable to search/export.
        crate::secrets::set_fail_scans_for_test(true);
        persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();
        crate::secrets::set_fail_scans_for_test(false);

        let scan_state: String = conn
            .query_row("SELECT scan_state FROM blob", [], |r| r.get(0))
            .unwrap();
        assert_eq!(scan_state, "failed_quarantined");

        let indexed: i64 = conn
            .query_row("SELECT count(*) FROM search_document", [], |r| r.get(0))
            .unwrap();
        assert_eq!(indexed, 0, "quarantined content is unavailable to search");

        let findings: i64 = conn
            .query_row("SELECT count(*) FROM secret_finding", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            findings, 0,
            "no findings are recorded for un-scanned content"
        );

        let sessions: i64 = conn
            .query_row("SELECT count(*) FROM agent_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            sessions, 1,
            "a scanner failure must not fail the whole session"
        );
    }
}
