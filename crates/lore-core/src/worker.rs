//! Background ingestion lifecycle: the seam that makes desktop ingestion
//! continuous.
//!
//! The M3 primitives — the debounced [`crate::watcher::SessionWatcher`], the
//! durable coalescing job queue ([`crate::jobs`]), and the bounded
//! [`crate::pipeline::Pipeline`] drain — are each testable in isolation. This
//! module binds them into a running system tied to the application lifecycle:
//!
//! - [`Worker`] owns a **dedicated** SQLite connection plus the adapter
//!   registry, blob store, and discovery config. It never shares the UI's
//!   connection, so background parsing/Git never blocks UI queries or holds the
//!   UI's database lock. Its methods are synchronous and deterministic so tests
//!   drive them without threads or sleeps.
//! - [`spawn`] runs a [`Worker`] on its own OS thread: recover interrupted jobs,
//!   run an initial incremental scan, then loop — poll the watcher, turn
//!   debounced paths into coalesced durable jobs, and drain a bounded batch —
//!   until told to stop. [`WorkerHandle::shutdown`] stops it deterministically
//!   and joins the thread; dropping the handle does the same.
//!
//! Everything the OS watcher callback does stays limited to filtering and
//! buffering (see [`crate::watcher`]); all database, parsing, Git, and progress
//! work happens on the worker thread, never inside an OS callback.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rusqlite::Connection;

use crate::adapters::AdapterRegistry;
use crate::discovery::DiscoveryConfig;
use crate::jobs::{self, QueueDepth, SourceSchedule};
use crate::pipeline::{DrainSummary, Pipeline, ProgressSink};
use crate::storage::blob::BlobStore;
use crate::watcher::SessionWatcher;

/// Tunables for the background worker. Defaults suit the desktop app; tests
/// override them (e.g. a large `idle_poll` so the loop blocks on the control
/// channel and no wall-clock time passes).
#[derive(Debug, Clone, Copy)]
pub struct WorkerConfig {
    /// Backpressure ceiling for runnable (pending + running) jobs.
    pub queue_capacity: usize,
    /// Upper bound on jobs drained per batch, keeping each pass bounded so one
    /// storm cannot monopolize the thread or hold a long transaction.
    pub drain_batch: usize,
    /// How long an idle loop waits for a control signal before polling the
    /// watcher again. Also bounds how soon a debounced change is drained.
    pub idle_poll: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 100_000,
            drain_batch: 256,
            idle_poll: Duration::from_millis(400),
        }
    }
}

/// Outcome of one [`Worker::run_pending`] pass over discovery/watcher input:
/// how many sources were newly enqueued and the drain tally that followed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WorkPass {
    pub enqueued: usize,
    pub drained: DrainSummary,
}

/// Owns the durable, filesystem-facing side of ingestion for one thread.
///
/// Holds no long-lived state beyond its collaborators; all durability lives in
/// SQLite, so a crash mid-run is recovered by [`Worker::recover`] on restart.
pub struct Worker {
    conn: Connection,
    registry: AdapterRegistry,
    blobs: BlobStore,
    config: DiscoveryConfig,
    cfg: WorkerConfig,
}

impl Worker {
    #[must_use]
    pub fn new(
        conn: Connection,
        registry: AdapterRegistry,
        blobs: BlobStore,
        config: DiscoveryConfig,
        cfg: WorkerConfig,
    ) -> Self {
        Self {
            conn,
            registry,
            blobs,
            config,
            cfg,
        }
    }

    fn pipeline(&self) -> Pipeline<'_> {
        Pipeline::new(
            &self.conn,
            &self.registry,
            &self.blobs,
            &self.config,
            self.cfg.queue_capacity,
        )
    }

    /// Return jobs left `running` by a terminated process to `pending`. Called
    /// once at startup so a restart resumes interrupted work rather than
    /// abandoning it. Returns the number of jobs recovered.
    pub fn recover(&self) -> jobs::Result<usize> {
        self.pipeline().recover()
    }

    /// Run a discovery pass, schedule an ingest job per discovered source
    /// (coalescing duplicates), then drain the queue in bounded batches until it
    /// is empty. Draining in batches keeps each transaction bounded and lets
    /// results land incrementally.
    pub fn scan(&self, sink: &dyn ProgressSink) -> jobs::Result<DrainSummary> {
        let pipeline = self.pipeline();
        pipeline.enqueue_scan(sink)?;
        let summary = self.drain_to_empty(&pipeline, sink)?;
        sink.emit(crate::pipeline::ProgressEvent::ScanFinished);
        Ok(summary)
    }

    /// Schedule ingest jobs for a batch of observed paths (e.g. debounced
    /// watcher output), resolving each path's owning adapter. Paths under no
    /// known root are ignored. Returns how many were newly enqueued (the rest
    /// coalesced onto an existing pending job or flagged a running one to redo).
    pub fn enqueue_paths(&self, paths: &[PathBuf]) -> jobs::Result<usize> {
        let pipeline = self.pipeline();
        let mut enqueued = 0;
        for path in paths {
            if let Some(SourceSchedule::Enqueued) = pipeline.enqueue_path(path)? {
                enqueued += 1;
            }
        }
        Ok(enqueued)
    }

    /// Enqueue `paths` then drain a single bounded batch. This is the per-tick
    /// unit of live ingestion: coalesce observed changes into durable jobs and
    /// make bounded forward progress.
    pub fn run_pending(
        &self,
        paths: &[PathBuf],
        sink: &dyn ProgressSink,
    ) -> jobs::Result<WorkPass> {
        let enqueued = self.enqueue_paths(paths)?;
        let drained = self.pipeline().drain(sink, self.cfg.drain_batch)?;
        Ok(WorkPass { enqueued, drained })
    }

    /// Drain one bounded batch of already-queued jobs.
    pub fn drain_batch(&self, sink: &dyn ProgressSink) -> jobs::Result<DrainSummary> {
        self.pipeline().drain(sink, self.cfg.drain_batch)
    }

    /// Observable backpressure: current pending/running job counts.
    pub fn queue_depth(&self) -> jobs::Result<QueueDepth> {
        jobs::queue_depth(&self.conn)
    }

    fn drain_to_empty(
        &self,
        pipeline: &Pipeline<'_>,
        sink: &dyn ProgressSink,
    ) -> jobs::Result<DrainSummary> {
        let mut total = DrainSummary::default();
        loop {
            let batch = pipeline.drain(sink, self.cfg.drain_batch)?;
            let made_progress = batch.processed() > 0 || batch.requeued > 0;
            total.ingested += batch.ingested;
            total.skipped += batch.skipped;
            total.failed += batch.failed;
            total.requeued += batch.requeued;
            total.enriched += batch.enriched;
            total.enrich_failed += batch.enrich_failed;
            if !made_progress {
                break;
            }
        }
        Ok(total)
    }
}

/// A control signal to the background worker thread.
struct Reconfiguration {
    config: DiscoveryConfig,
    watcher: Option<SessionWatcher>,
}

enum Signal {
    /// Run a full discovery→ingest scan (e.g. the app's manual "Rescan").
    Rescan,
    /// Wake up now and process any pending watcher/queue work.
    Wake,
    /// Replace the discovery roots and watcher, then scan the new roots. This
    /// lets the desktop add/remove custom folders without restarting Lore.
    Reconfigure(Box<Reconfiguration>),
    /// Stop after the current bounded step and let the thread exit.
    Shutdown,
}

/// Handle to a running background worker. Dropping it shuts the worker down and
/// joins its thread, so the worker never outlives the handle.
pub struct WorkerHandle {
    tx: Sender<Signal>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// Ask the worker to run a full rescan. Non-blocking; returns immediately.
    pub fn trigger_rescan(&self) {
        let _ = self.tx.send(Signal::Rescan);
    }

    /// Nudge the worker to poll the watcher and drain now instead of waiting for
    /// its next idle tick. Non-blocking.
    pub fn wake(&self) {
        let _ = self.tx.send(Signal::Wake);
    }

    /// Replace the worker's discovery roots and live watcher, then run an
    /// incremental scan. Non-blocking; progress uses the normal content-free
    /// events and all source access remains read-only.
    pub fn reconfigure(&self, config: DiscoveryConfig, watcher: Option<SessionWatcher>) {
        let _ = self.tx.send(Signal::Reconfigure(Box::new(Reconfiguration {
            config,
            watcher,
        })));
    }

    /// Stop the worker and join its thread. Deterministic: the worker finishes
    /// its current bounded step, then exits; interrupted work stays durable and
    /// is recovered on the next start. Safe to call once.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.tx.send(Signal::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run `worker` on a dedicated thread and return a handle to control it.
///
/// The thread: recovers interrupted jobs, runs an initial scan, then loops —
/// blocking for a control signal (up to `idle_poll`), polling the watcher, and
/// draining a bounded batch — until shutdown. A `None` watcher means "no live
/// updates" (still useful for the initial scan and manual rescans).
///
/// Errors from any single step are swallowed so one failure never tears down
/// the whole worker; per-source failures are already recorded durably by the
/// pipeline, and infrastructure errors surface on the next pass.
pub fn spawn<S>(mut worker: Worker, mut watcher: Option<SessionWatcher>, sink: S) -> WorkerHandle
where
    S: ProgressSink + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let idle_poll = worker.cfg.idle_poll;
    let join = thread::spawn(move || {
        // Startup: recover interrupted work, then an initial incremental scan.
        let _ = worker.recover();
        let _ = worker.scan(&sink);

        loop {
            let signal = rx.recv_timeout(idle_poll);
            match signal {
                Ok(Signal::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Signal::Rescan) => {
                    let _ = worker.scan(&sink);
                }
                Ok(Signal::Reconfigure(next)) => {
                    let Reconfiguration {
                        config,
                        watcher: next_watcher,
                    } = *next;
                    worker.config = config;
                    watcher = next_watcher;
                    let _ = worker.scan(&sink);
                }
                Ok(Signal::Wake) | Err(RecvTimeoutError::Timeout) => {}
            }
            let ready = poll_ready(watcher.as_mut());
            let _ = worker.run_pending(&ready, &sink);
        }
    });
    WorkerHandle {
        tx,
        join: Some(join),
    }
}

fn poll_ready(watcher: Option<&mut SessionWatcher>) -> Vec<PathBuf> {
    watcher.map(|w| w.poll().ready_paths).unwrap_or_default()
}

/// Convenience: open a dedicated worker connection at `db_path` and build a
/// [`Worker`]. The connection is independent of any UI connection to the same
/// file; WAL lets them read/write concurrently.
pub fn open_worker(
    db_path: &Path,
    registry: AdapterRegistry,
    blobs: BlobStore,
    config: DiscoveryConfig,
    cfg: WorkerConfig,
) -> crate::storage::Result<Worker> {
    let conn = crate::storage::open(db_path)?;
    Ok(Worker::new(conn, registry, blobs, config, cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::NullSink;

    #[test]
    fn default_config_is_bounded() {
        let cfg = WorkerConfig::default();
        assert!(cfg.drain_batch > 0);
        assert!(cfg.queue_capacity >= cfg.drain_batch);
    }

    #[test]
    fn drain_to_empty_on_no_work_is_a_noop() {
        let conn = crate::storage::open_in_memory().unwrap();
        let worker = Worker::new(
            conn,
            AdapterRegistry::v0(),
            BlobStore::open(tempfile::tempdir().unwrap().path()).unwrap(),
            DiscoveryConfig::new(),
            WorkerConfig::default(),
        );
        let summary = worker.drain_batch(&NullSink).unwrap();
        assert_eq!(summary, DrainSummary::default());
        assert_eq!(worker.queue_depth().unwrap(), QueueDepth::default());
    }
}
