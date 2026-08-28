//! Ingest pipeline: turn discovery results and watcher events into durable
//! per-source jobs, then drain them through bounded, transactionally-isolated
//! ingest with content-free progress.
//!
//! This is the M3 seam between the filesystem-facing layers (discovery, watcher)
//! and persistence (ingest). It owns no threads: `drain` is a deterministic,
//! bounded claim→ingest→finish loop so tests can inject events without sleeps,
//! and a real app can call it from one or more worker threads (each on its own
//! connection) without changing the durable contract. Progress events are
//! content-free — an adapter id, an outcome, and counts — so they can be relayed
//! straight to Tauri IPC without leaking session content or paths.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, ErrorCode, OptionalExtension};
use serde_json::Value;

use crate::adapters::{AdapterRegistry, SessionRef};
use crate::discovery::{discover, owner_of, DiscoveryConfig};
use crate::ingest::{ingest_file, ChangeClass, IngestFailureKind, IngestOutcome};
use crate::jobs::{self, FinishOutcome, NewJob, SourceSchedule};
use crate::storage::blob::BlobStore;
use crate::storage::StorageError;

/// Durable job kind for a single source-file ingest.
const JOB_KIND: &str = "ingest_source";
/// Durable job kind for a coalesced commit re-verification pass (I2).
const JOB_KIND_REVERIFY: &str = "reverify";
/// Re-verification runs at the lowest priority so it never jumps ahead of
/// transcript ingest; it must never block transcript reads or search
/// (`GIT_INTEGRATION.md` §8).
const REVERIFY_PRIORITY: i64 = 0;
/// Must advance whenever parser output changes so terminal jobs are reconsidered.
/// Keep aligned with the ingest checkpoint parser version.
const JOB_PARSER_VERSION: &str = "2";

/// A content-free progress event. Carries an adapter id (a static schema
/// identifier), an outcome, and counts — never a path or session content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    /// A scan pass scheduled work: `discovered` sources seen, `enqueued` newly
    /// queued (the rest coalesced onto existing jobs).
    ScanEnqueued { discovered: usize, enqueued: usize },
    /// A full discovery scan has drained all work it scheduled.
    ScanFinished,
    /// A source was ingested with the given change classification.
    Ingested {
        agent_id: String,
        session_id: String,
        change: ChangeClass,
    },
    /// A source was unchanged and skipped.
    Skipped { agent_id: String },
    /// A source failed to ingest; peers are unaffected.
    Failed {
        agent_id: String,
        kind: IngestFailureKind,
    },
    /// A change arrived while the source's job ran; it was re-queued.
    Requeued { agent_id: String },
}

/// Receiver for [`ProgressEvent`]s. Implementations must not block the pipeline.
pub trait ProgressSink {
    fn emit(&self, event: ProgressEvent);
}

/// A sink that discards every event.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl ProgressSink for NullSink {
    fn emit(&self, _event: ProgressEvent) {}
}

/// Summary of one [`Pipeline::drain`] pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DrainSummary {
    pub ingested: usize,
    pub skipped: usize,
    pub failed: usize,
    pub requeued: usize,
    /// Segments linked to a repository by post-ingest git enrichment.
    pub enriched: usize,
    /// Ingests whose (best-effort) enrichment errored. The session is still
    /// persisted; enrichment can be retried on a later ingest.
    pub enrich_failed: usize,
    /// Coalesced commit re-verification jobs drained this pass (I2).
    pub reverified: usize,
}

impl DrainSummary {
    #[must_use]
    pub fn processed(&self) -> usize {
        self.ingested + self.skipped + self.failed + self.reverified
    }
}

/// The ingest coordinator. Borrows its collaborators; holds no long-lived state
/// of its own (durability lives in SQLite).
pub struct Pipeline<'a> {
    conn: &'a Connection,
    registry: &'a AdapterRegistry,
    blobs: &'a BlobStore,
    config: &'a DiscoveryConfig,
    capacity: usize,
}

impl<'a> Pipeline<'a> {
    #[must_use]
    pub fn new(
        conn: &'a Connection,
        registry: &'a AdapterRegistry,
        blobs: &'a BlobStore,
        config: &'a DiscoveryConfig,
        capacity: usize,
    ) -> Self {
        Self {
            conn,
            registry,
            blobs,
            config,
            capacity,
        }
    }

    /// Return jobs left `running` by a terminated process to `pending` so a
    /// restart resumes rather than abandons them.
    pub fn recover(&self) -> jobs::Result<usize> {
        jobs::recover_running(self.conn)
    }

    /// Run a discovery pass and schedule an ingest job per discovered source,
    /// coalescing duplicates. Returns the number newly enqueued.
    pub fn enqueue_scan(&self, sink: &dyn ProgressSink) -> jobs::Result<usize> {
        let report = discover(self.registry, self.config);
        let discovered = report.sessions.len();
        let mut enqueued = 0;
        for source in &report.sessions {
            if self.schedule(source.agent.0, &source.path)? == SourceSchedule::Enqueued {
                enqueued += 1;
            }
        }
        self.reconcile_source_presence(&report.sessions)?;
        sink.emit(ProgressEvent::ScanEnqueued {
            discovered,
            enqueued,
        });
        Ok(enqueued)
    }

    /// Schedule an ingest job for a single observed path (e.g. from the
    /// watcher), resolving its owning adapter. Returns `None` when no adapter
    /// owns the path.
    pub fn enqueue_path(&self, path: &Path) -> jobs::Result<Option<SourceSchedule>> {
        let Some(agent) = owner_of(self.registry, self.config, path) else {
            return Ok(None);
        };
        Ok(Some(self.schedule(agent.0, path)?))
    }

    /// Schedule one low-priority re-verification job per distinct recorded
    /// `(worktree, commit)` pair, coalescing by a stable id (I2). A finished job
    /// is re-armed so each trigger re-checks the live repository; a
    /// pending/running job is left alone. Returns the number newly enqueued.
    pub fn enqueue_reverify(&self) -> jobs::Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT s.worktree_id, o.commit_sha
             FROM session_segment s
             JOIN git_observation o ON o.segment_id = s.id
                AND o.source = 'agent_recorded' AND o.commit_sha IS NOT NULL
             WHERE s.worktree_id IS NOT NULL",
        )?;
        let pairs: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut enqueued = 0usize;
        for (worktree_id, commit_sha) in pairs {
            let id = reverify_job_id(&worktree_id, &commit_sha);
            let payload = serde_json::json!({
                "worktree_id": worktree_id,
                "commit_sha": commit_sha,
            })
            .to_string();
            let state: Option<String> = self
                .conn
                .query_row("SELECT state FROM job WHERE id = ?1", [&id], |r| r.get(0))
                .optional()?;
            match state.as_deref() {
                None => {
                    jobs::enqueue(
                        self.conn,
                        &NewJob {
                            id: &id,
                            kind: JOB_KIND_REVERIFY,
                            priority: REVERIFY_PRIORITY,
                            payload_json: Some(&payload),
                        },
                        self.capacity,
                    )?;
                    enqueued += 1;
                }
                Some("pending") | Some("running") => { /* coalesce onto existing work */ }
                Some("done" | "failed") => {
                    self.conn.execute(
                        "UPDATE job SET state = 'pending', redo = 0, error = NULL,
                                error_kind = NULL, attempts = 0, priority = ?2,
                                payload_json = ?3, updated_at = unixepoch('now')*1000
                         WHERE id = ?1",
                        params![id, REVERIFY_PRIORITY, payload],
                    )?;
                    enqueued += 1;
                }
                Some(_) => {}
            }
        }
        Ok(enqueued)
    }

    fn schedule(&self, agent_id: &str, path: &Path) -> jobs::Result<SourceSchedule> {
        let id = job_id(agent_id, path);
        let metadata = std::fs::metadata(path).ok();
        // Prefer recent sessions during a large first scan so the archive
        // becomes useful immediately instead of waiting behind months-old,
        // potentially multi-megabyte histories. The path remains the stable
        // coalescing identity; mtime only controls claim order.
        let priority = metadata
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
            .unwrap_or(0);
        let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);
        let mtime_ns = metadata
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_nanos().to_string())
            .unwrap_or_default();
        let payload = encode_payload(agent_id, path, size, &mtime_ns);
        jobs::schedule_source(
            self.conn,
            &NewJob {
                id: &id,
                kind: JOB_KIND,
                priority,
                payload_json: Some(&payload),
            },
            self.capacity,
        )
    }

    /// Reconcile `source_artifact.state` against a fresh discovery pass: an
    /// artifact whose file has disappeared from a still-readable, non-empty root
    /// is marked `missing` (with `last_seen_at` bumped); one that reappeared is
    /// restored to `active`. A root that is absent, unreadable, or empty is
    /// skipped entirely, so a transient failure (unmounted volume, lost
    /// permission, removed custom root) never mass-marks its artifacts missing
    /// (N7a). Returns the number of rows whose state changed.
    fn reconcile_source_presence(&self, discovered: &[SessionRef]) -> rusqlite::Result<usize> {
        let now_ms = || -> i64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        };
        let mut changed = 0usize;

        for adapter in self.registry.iter() {
            let agent_id = adapter.id().0;
            let mut roots = adapter.roots(&self.config.roots_for(agent_id));
            roots.sort();
            roots.dedup();

            let present: BTreeSet<&Path> = discovered
                .iter()
                .filter(|s| s.agent.0 == agent_id)
                .map(|s| s.path.as_path())
                .collect();

            for root in roots {
                if !root_readable_and_nonempty(&root) {
                    continue;
                }
                let mut stmt = self.conn.prepare(
                    "SELECT id, current_path, state FROM source_artifact WHERE agent_id = ?1",
                )?;
                let rows: Vec<(String, String, String)> = stmt
                    .query_map([agent_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for (id, current_path, state) in rows {
                    let cp = Path::new(&current_path);
                    if !cp.starts_with(&root) {
                        // Under a root we no longer manage; not ours to judge.
                        continue;
                    }
                    let next = if present.contains(cp) {
                        "active"
                    } else {
                        "missing"
                    };
                    if next != state {
                        self.conn.execute(
                            "UPDATE source_artifact
                             SET state = ?2, last_seen_at = ?3 WHERE id = ?1",
                            params![id, next, now_ms()],
                        )?;
                        changed += 1;
                    }
                }
            }
        }
        Ok(changed)
    }

    /// Claim and process up to `max` pending jobs. Each source ingests in its
    /// own transaction (one failure never stops peers) and the job completes
    /// redo-aware, so a change observed mid-run reprocesses. Progress is emitted
    /// per source. Infrastructure errors propagate; per-source ingest failures
    /// are recorded, not propagated.
    pub fn drain(&self, sink: &dyn ProgressSink, max: usize) -> jobs::Result<DrainSummary> {
        let mut summary = DrainSummary::default();
        for _ in 0..max {
            let Some(job) = jobs::claim_next(self.conn)? else {
                break;
            };
            if job.kind == JOB_KIND_REVERIFY {
                let Some((worktree_id, commit_sha)) = job
                    .payload_json
                    .as_deref()
                    .and_then(decode_reverify_payload)
                else {
                    jobs::fail_with_kind(
                        self.conn,
                        &job.id,
                        "invalid_payload",
                        "unreadable reverify job payload",
                    )?;
                    summary.reverified += 1;
                    continue;
                };
                match crate::enrich::reverify_commit(self.conn, &worktree_id, &commit_sha) {
                    Ok(_) => jobs::complete(self.conn, &job.id)?,
                    Err(error) => jobs::fail_with_kind(
                        self.conn,
                        &job.id,
                        "reverify_failed",
                        &error.to_string(),
                    )?,
                }
                summary.reverified += 1;
                continue;
            }
            let Some((agent_id, path)) = job.payload_json.as_deref().and_then(decode_payload)
            else {
                jobs::fail_with_kind(
                    self.conn,
                    &job.id,
                    "invalid_payload",
                    "unreadable ingest job payload",
                )?;
                summary.failed += 1;
                continue;
            };
            let Some(adapter) = self.registry.get(&agent_id) else {
                jobs::fail_with_kind(
                    self.conn,
                    &job.id,
                    "adapter_not_registered",
                    "adapter not registered",
                )?;
                sink.emit(ProgressEvent::Failed {
                    agent_id,
                    kind: IngestFailureKind::AdapterNotRegistered,
                });
                summary.failed += 1;
                continue;
            };

            match ingest_file(self.conn, adapter, &path, self.blobs) {
                Ok(outcome) => {
                    match outcome {
                        IngestOutcome::Skipped => {
                            summary.skipped += 1;
                            sink.emit(ProgressEvent::Skipped {
                                agent_id: agent_id.clone(),
                            });
                        }
                        IngestOutcome::Ingested {
                            change,
                            ref session_id,
                            ..
                        } => {
                            summary.ingested += 1;
                            sink.emit(ProgressEvent::Ingested {
                                agent_id: agent_id.clone(),
                                session_id: session_id.clone(),
                                change,
                            });
                            // Best-effort git enrichment: a failure never undoes
                            // the committed session (it can be retried later).
                            match crate::enrich::enrich_session(self.conn, session_id) {
                                Ok(n) => summary.enriched += n,
                                Err(_) => summary.enrich_failed += 1,
                            }
                        }
                    }
                    if jobs::finish(self.conn, &job.id)? == FinishOutcome::Requeued {
                        summary.requeued += 1;
                        sink.emit(ProgressEvent::Requeued { agent_id });
                    }
                }
                Err(error) => {
                    // Storage errors are deliberately content-free, so keeping
                    // their category makes failures diagnosable without
                    // exposing source text or paths.
                    jobs::fail_with_kind(
                        self.conn,
                        &job.id,
                        ingest_failure_kind(&error),
                        &error.to_string(),
                    )?;
                    summary.failed += 1;
                    sink.emit(ProgressEvent::Failed {
                        agent_id,
                        kind: IngestFailureKind::SourceFailed,
                    });
                }
            }
        }
        Ok(summary)
    }
}

fn ingest_failure_kind(error: &StorageError) -> &'static str {
    match error {
        StorageError::Io => "source_io",
        StorageError::Migration(_) => "storage_migration",
        StorageError::Sqlite(error) => match error.sqlite_error_code() {
            Some(ErrorCode::ConstraintViolation) => "sqlite_constraint",
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => "sqlite_busy",
            _ => "sqlite",
        },
    }
}

/// Stable, compact job id so repeated events for one source coalesce onto one
/// job. Derived from the adapter id and the path; the raw path is not the id.
fn job_id(agent_id: &str, path: &Path) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut mix = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    mix(agent_id.as_bytes());
    mix(&[0x1f]);
    mix(path.to_string_lossy().as_bytes());
    format!("ingest_{hash:016x}")
}

fn encode_payload(agent_id: &str, path: &Path, size: u64, mtime_ns: &str) -> String {
    serde_json::json!({
        "agent_id": agent_id,
        "path": path.to_string_lossy(),
        "size": size,
        "mtime_ns": mtime_ns,
        "parser_version": JOB_PARSER_VERSION,
    })
    .to_string()
}

fn decode_payload(payload: &str) -> Option<(String, PathBuf)> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let agent_id = value.get("agent_id")?.as_str()?.to_string();
    let path = value.get("path")?.as_str()?;
    Some((agent_id, PathBuf::from(path)))
}

/// Stable, compact job id for a `(worktree, commit)` re-verification unit, so
/// re-triggering coalesces onto the same job (I2).
fn reverify_job_id(worktree_id: &str, commit_sha: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut mix = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    mix(worktree_id.as_bytes());
    mix(&[0x1f]);
    mix(commit_sha.as_bytes());
    format!("reverify_{hash:016x}")
}

fn decode_reverify_payload(payload: &str) -> Option<(String, String)> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let worktree_id = value.get("worktree_id")?.as_str()?.to_string();
    let commit_sha = value.get("commit_sha")?.as_str()?.to_string();
    Some((worktree_id, commit_sha))
}

/// A root we are allowed to draw conclusions from: it exists, is readable, and
/// has at least one entry. An unmounted volume commonly appears as an existing
/// but empty directory, so emptiness is treated as "don't know" rather than
/// "genuinely empty" — the same conservative stance as an absent root (N7a).
fn root_readable_and_nonempty(root: &Path) -> bool {
    match std::fs::read_dir(root) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_id_is_stable_and_source_specific() {
        let a = Path::new("/roots/claude/s1.jsonl");
        let b = Path::new("/roots/claude/s2.jsonl");
        assert_eq!(job_id("claude-code", a), job_id("claude-code", a));
        assert_ne!(job_id("claude-code", a), job_id("claude-code", b));
        assert_ne!(job_id("claude-code", a), job_id("codex", a));
    }

    #[test]
    fn payload_round_trips() {
        let encoded = encode_payload("codex", Path::new("/x/rollout.jsonl"), 42, "1234");
        let (agent, path) = decode_payload(&encoded).unwrap();
        assert_eq!(agent, "codex");
        assert_eq!(path, PathBuf::from("/x/rollout.jsonl"));
        assert!(decode_payload("not json").is_none());
    }
}
