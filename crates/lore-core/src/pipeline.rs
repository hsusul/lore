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

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use crate::adapters::AdapterRegistry;
use crate::discovery::{discover, owner_of, DiscoveryConfig};
use crate::ingest::{ingest_file, ChangeClass, IngestFailureKind, IngestOutcome};
use crate::jobs::{self, FinishOutcome, NewJob, SourceSchedule};
use crate::storage::blob::BlobStore;

/// Durable job kind for a single source-file ingest.
const JOB_KIND: &str = "ingest_source";

/// A content-free progress event. Carries an adapter id (a static schema
/// identifier), an outcome, and counts — never a path or session content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    /// A scan pass scheduled work: `discovered` sources seen, `enqueued` newly
    /// queued (the rest coalesced onto existing jobs).
    ScanEnqueued { discovered: usize, enqueued: usize },
    /// A source was ingested with the given change classification.
    Ingested {
        agent_id: String,
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
}

impl DrainSummary {
    #[must_use]
    pub fn processed(&self) -> usize {
        self.ingested + self.skipped + self.failed
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

    fn schedule(&self, agent_id: &str, path: &Path) -> jobs::Result<SourceSchedule> {
        let id = job_id(agent_id, path);
        let payload = encode_payload(agent_id, path);
        jobs::schedule_source(
            self.conn,
            &NewJob {
                id: &id,
                kind: JOB_KIND,
                priority: 0,
                payload_json: Some(&payload),
            },
            self.capacity,
        )
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
            let Some((agent_id, path)) = job.payload_json.as_deref().and_then(decode_payload)
            else {
                jobs::fail(self.conn, &job.id, "unreadable ingest job payload")?;
                summary.failed += 1;
                continue;
            };
            let Some(adapter) = self.registry.get(&agent_id) else {
                jobs::fail(self.conn, &job.id, "adapter not registered")?;
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
                        IngestOutcome::Ingested { change, .. } => {
                            summary.ingested += 1;
                            sink.emit(ProgressEvent::Ingested {
                                agent_id: agent_id.clone(),
                                change,
                            });
                        }
                    }
                    if jobs::finish(self.conn, &job.id)? == FinishOutcome::Requeued {
                        summary.requeued += 1;
                        sink.emit(ProgressEvent::Requeued { agent_id });
                    }
                }
                Err(_) => {
                    jobs::fail(self.conn, &job.id, "source ingest failed")?;
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

fn encode_payload(agent_id: &str, path: &Path) -> String {
    serde_json::json!({ "agent_id": agent_id, "path": path.to_string_lossy() }).to_string()
}

fn decode_payload(payload: &str) -> Option<(String, PathBuf)> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let agent_id = value.get("agent_id")?.as_str()?.to_string();
    let path = value.get("path")?.as_str()?;
    Some((agent_id, PathBuf::from(path)))
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
        let encoded = encode_payload("codex", Path::new("/x/rollout.jsonl"));
        let (agent, path) = decode_payload(&encoded).unwrap();
        assert_eq!(agent, "codex");
        assert_eq!(path, PathBuf::from("/x/rollout.jsonl"));
        assert!(decode_payload("not json").is_none());
    }
}
