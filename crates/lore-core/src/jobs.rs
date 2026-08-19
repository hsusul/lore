//! Durable bounded job queue with backpressure.
//!
//! The queue is persisted in SQLite so process restarts can return interrupted
//! work to `pending`. Claiming is transactional and respects priority then FIFO.
//! Worker threads live in [`crate::worker`]; progress events are an app-layer
//! concern surfaced through the Tauri shell.

use rusqlite::{params, Connection, OptionalExtension};

/// A queued background task. Payloads are versioned JSON owned by the task kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub priority: i64,
    pub state: JobState,
    pub attempts: i64,
    pub payload_json: Option<String>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Done,
    Failed,
}

impl JobState {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            _ => Err(JobQueueError::InvalidState),
        }
    }
}

/// New durable task. Callers choose a stable id when duplicate scheduling
/// should coalesce; an already-present id returns `Duplicate` without mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub priority: i64,
    pub payload_json: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Enqueued,
    Duplicate,
}

#[derive(Debug, thiserror::Error)]
pub enum JobQueueError {
    #[error("sqlite job queue error")]
    Sqlite(#[from] rusqlite::Error),
    #[error("job queue is at its configured capacity of {limit}")]
    Full { limit: usize },
    #[error("job row has an invalid state")]
    InvalidState,
    #[error("job is not running")]
    NotRunning,
}

pub type Result<T> = std::result::Result<T, JobQueueError>;

/// Insert a task unless its id is already durable. Pending + running rows count
/// toward `capacity`; completed history does not consume backpressure capacity.
pub fn enqueue(conn: &Connection, job: &NewJob<'_>, capacity: usize) -> Result<EnqueueOutcome> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM job WHERE id = ?1)",
        [job.id],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(EnqueueOutcome::Duplicate);
    }

    let active: i64 = conn.query_row(
        "SELECT count(*) FROM job WHERE state IN ('pending','running')",
        [],
        |row| row.get(0),
    )?;
    if usize::try_from(active).unwrap_or(usize::MAX) >= capacity {
        return Err(JobQueueError::Full { limit: capacity });
    }

    conn.execute(
        "INSERT INTO job
            (id, kind, priority, state, attempts, payload_json, created_at, updated_at)
         VALUES (?1,?2,?3,'pending',0,?4,unixepoch('now')*1000,unixepoch('now')*1000)",
        params![job.id, job.kind, job.priority, job.payload_json],
    )?;
    Ok(EnqueueOutcome::Enqueued)
}

/// Outcome of scheduling a source-ingest task whose id coalesces repeated
/// filesystem events for the same source (see [`schedule_source`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceSchedule {
    /// Newly queued: no job existed, or a previously finished job was re-armed.
    Enqueued,
    /// An identical task is already pending; the storm coalesced onto it.
    CoalescedPending,
    /// A failed run already covers this exact source fingerprint, so it stays
    /// failed until the source or parser version changes.
    CoalescedFailed,
    /// A successful run already covers this exact source fingerprint.
    CoalescedDone,
    /// A task for this source is running now; it was flagged to re-run on finish
    /// so the change that arrived mid-run is not lost.
    MarkedRedo,
}

/// Redo-aware completion outcome for a source-ingest task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishOutcome {
    /// Marked done.
    Done,
    /// A change arrived while the task ran; it was returned to `pending`.
    Requeued,
}

/// Schedule an ingest task for a source, coalescing duplicate filesystem events.
///
/// The job `id` is expected to be a stable function of the source identity, so
/// repeated events for the same source converge:
/// - no row yet → enqueue (subject to `capacity`);
/// - a `pending` row → coalesce (the queued run will pick up the latest bytes);
/// - a `running` row → set `redo` so it re-runs on completion (the mid-run
///   change is never dropped);
/// - a finished (`done`/`failed`) row → re-arm it to `pending`.
pub fn schedule_source(
    conn: &Connection,
    job: &NewJob<'_>,
    capacity: usize,
) -> Result<SourceSchedule> {
    let existing: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT state, payload_json FROM job WHERE id = ?1",
            [job.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match existing.as_ref().map(|(state, _)| state.as_str()) {
        None => {
            enqueue(conn, job, capacity)?;
            Ok(SourceSchedule::Enqueued)
        }
        Some("pending") => {
            // A later discovery pass may have better scheduling metadata (for
            // example, a file was appended and now has a newer mtime). Refresh
            // the pending row without creating duplicate work.
            conn.execute(
                "UPDATE job SET priority = ?2, payload_json = ?3,
                                updated_at = unixepoch('now')*1000
                 WHERE id = ?1 AND state = 'pending'",
                params![job.id, job.priority, job.payload_json],
            )?;
            Ok(SourceSchedule::CoalescedPending)
        }
        Some("running") => {
            conn.execute(
                "UPDATE job SET redo = 1, updated_at = unixepoch('now')*1000
                 WHERE id = ?1 AND state = 'running'",
                [job.id],
            )?;
            Ok(SourceSchedule::MarkedRedo)
        }
        Some("done")
            if existing
                .as_ref()
                .is_some_and(|(_, payload)| payload.as_deref() == job.payload_json) =>
        {
            Ok(SourceSchedule::CoalescedDone)
        }
        Some("failed")
            if existing
                .as_ref()
                .is_some_and(|(_, payload)| payload.as_deref() == job.payload_json) =>
        {
            Ok(SourceSchedule::CoalescedFailed)
        }
        Some("done" | "failed") => {
            // Re-arm the finished job for another run with the latest payload.
            conn.execute(
                "UPDATE job SET state = 'pending', redo = 0, error = NULL, error_kind = NULL,
                                attempts = 0,
                                priority = ?2, payload_json = ?3,
                                updated_at = unixepoch('now')*1000
                 WHERE id = ?1",
                params![job.id, job.priority, job.payload_json],
            )?;
            Ok(SourceSchedule::Enqueued)
        }
        Some(_) => Err(JobQueueError::InvalidState),
    }
}

/// Complete a claimed task, honoring a `redo` flag set while it ran: if the
/// source changed mid-run the task returns to `pending` instead of `done`.
pub fn finish(conn: &Connection, id: &str) -> Result<FinishOutcome> {
    let redo: Option<i64> = conn
        .query_row(
            "SELECT redo FROM job WHERE id = ?1 AND state = 'running'",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(redo) = redo else {
        return Err(JobQueueError::NotRunning);
    };
    if redo != 0 {
        conn.execute(
            "UPDATE job SET state = 'pending', redo = 0, error = NULL, error_kind = NULL,
                            updated_at = unixepoch('now')*1000
             WHERE id = ?1",
            [id],
        )?;
        Ok(FinishOutcome::Requeued)
    } else {
        conn.execute(
            "UPDATE job SET state = 'done', updated_at = unixepoch('now')*1000
             WHERE id = ?1",
            [id],
        )?;
        Ok(FinishOutcome::Done)
    }
}

/// Atomically claim the highest-priority oldest pending task.
pub fn claim_next(conn: &Connection) -> Result<Option<Job>> {
    let tx = conn.unchecked_transaction()?;
    let id: Option<String> = tx
        .query_row(
            "SELECT id FROM job WHERE state = 'pending'
             ORDER BY priority DESC, created_at ASC, id ASC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        tx.commit()?;
        return Ok(None);
    };

    let changed = tx.execute(
        "UPDATE job SET state = 'running', attempts = attempts + 1,
                        error = NULL, error_kind = NULL,
                        updated_at = unixepoch('now')*1000
         WHERE id = ?1 AND state = 'pending'",
        [&id],
    )?;
    if changed != 1 {
        return Ok(None);
    }
    let job = load_job(&tx, &id)?.ok_or(JobQueueError::InvalidState)?;
    tx.commit()?;
    Ok(Some(job))
}

/// Mark a claimed task complete.
pub fn complete(conn: &Connection, id: &str) -> Result<()> {
    transition_running(conn, id, "done", None, None)
}

/// Mark a claimed task failed with a bounded, caller-scrubbed diagnostic.
pub fn fail(conn: &Connection, id: &str, error: &str) -> Result<()> {
    fail_with_kind(conn, id, "unknown", error)
}

/// Mark a claimed task failed with a stable category and bounded diagnostic.
pub fn fail_with_kind(conn: &Connection, id: &str, kind: &str, error: &str) -> Result<()> {
    let bounded: String = error.chars().take(500).collect();
    let bounded_kind: String = kind.chars().take(80).collect();
    transition_running(conn, id, "failed", Some(&bounded_kind), Some(&bounded))
}

/// Observable backpressure snapshot: how much runnable work the durable queue
/// holds right now. `pending + running` is what [`enqueue`]/[`schedule_source`]
/// weigh against the configured capacity, so surfacing it lets the app report
/// queue depth without exposing any job payload or source path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueDepth {
    pub pending: usize,
    pub running: usize,
}

impl QueueDepth {
    /// Rows that count toward the backpressure ceiling.
    #[must_use]
    pub fn active(&self) -> usize {
        self.pending + self.running
    }
}

/// Read the current [`QueueDepth`]. Content-free: counts only.
pub fn queue_depth(conn: &Connection) -> Result<QueueDepth> {
    let (pending, running) = conn.query_row(
        "SELECT
            count(*) FILTER (WHERE state = 'pending'),
            count(*) FILTER (WHERE state = 'running')
         FROM job",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    Ok(QueueDepth {
        pending: usize::try_from(pending).unwrap_or(usize::MAX),
        running: usize::try_from(running).unwrap_or(usize::MAX),
    })
}

/// Maximum consecutive execution attempts before a repeatedly crashed job is
/// marked failed to prevent infinite restart crash loops.
pub const MAX_JOB_ATTEMPTS: i64 = 5;

/// Return tasks left `running` by a terminated process to `pending`, unless
/// they have exceeded `MAX_JOB_ATTEMPTS` (in which case they are marked `failed`
/// with `error_kind = 'poisoned'` to prevent restart crash loops).
pub fn recover_running(conn: &Connection) -> Result<usize> {
    conn.execute(
        "UPDATE job SET state = 'failed', error_kind = 'poisoned',
                        error = 'job exceeded maximum crash recovery attempts',
                        updated_at = unixepoch('now')*1000
         WHERE state = 'running' AND attempts >= ?1",
        [MAX_JOB_ATTEMPTS],
    )?;
    let changed = conn.execute(
        "UPDATE job SET state = 'pending', updated_at = unixepoch('now')*1000
         WHERE state = 'running'",
        [],
    )?;
    Ok(changed)
}

pub fn load(conn: &Connection, id: &str) -> Result<Option<Job>> {
    load_job(conn, id)
}

fn transition_running(
    conn: &Connection,
    id: &str,
    target: &str,
    error_kind: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    let changed = conn.execute(
        "UPDATE job SET state = ?2, error_kind = ?3, error = ?4,
                        updated_at = unixepoch('now')*1000
         WHERE id = ?1 AND state = 'running'",
        params![id, target, error_kind, error],
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(JobQueueError::NotRunning)
    }
}

fn load_job(conn: &Connection, id: &str) -> Result<Option<Job>> {
    conn.query_row(
        "SELECT id, kind, priority, state, attempts, payload_json, error, error_kind,
                created_at, updated_at
         FROM job WHERE id = ?1",
        [id],
        |row| {
            let state: String = row.get(3)?;
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                state,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        },
    )
    .optional()?
    .map(
        |(
            id,
            kind,
            priority,
            state,
            attempts,
            payload_json,
            error,
            error_kind,
            created_at,
            updated_at,
        )| {
            Ok(Job {
                id,
                kind,
                priority,
                state: JobState::parse(&state)?,
                attempts,
                payload_json,
                error,
                error_kind,
                created_at,
                updated_at,
            })
        },
    )
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        crate::storage::open_in_memory().unwrap()
    }

    fn new_job<'a>(id: &'a str, priority: i64) -> NewJob<'a> {
        NewJob {
            id,
            kind: "ingest_source",
            priority,
            payload_json: Some(r#"{"source":"fixture"}"#),
        }
    }

    #[test]
    fn claims_by_priority_then_fifo_id_and_completes() {
        let conn = db();
        enqueue(&conn, &new_job("low", 0), 10).unwrap();
        enqueue(&conn, &new_job("high-b", 10), 10).unwrap();
        enqueue(&conn, &new_job("high-a", 10), 10).unwrap();

        let first = claim_next(&conn).unwrap().unwrap();
        assert_eq!(first.id, "high-a");
        assert_eq!(first.state, JobState::Running);
        assert_eq!(first.attempts, 1);
        complete(&conn, &first.id).unwrap();
        assert_eq!(
            load(&conn, &first.id).unwrap().unwrap().state,
            JobState::Done
        );

        assert_eq!(claim_next(&conn).unwrap().unwrap().id, "high-b");
        assert_eq!(claim_next(&conn).unwrap().unwrap().id, "low");
        assert!(claim_next(&conn).unwrap().is_none());
    }

    #[test]
    fn duplicate_ids_coalesce_before_capacity_check() {
        let conn = db();
        assert_eq!(
            enqueue(&conn, &new_job("same", 0), 1).unwrap(),
            EnqueueOutcome::Enqueued
        );
        assert_eq!(
            enqueue(&conn, &new_job("same", 99), 1).unwrap(),
            EnqueueOutcome::Duplicate
        );
        assert!(matches!(
            enqueue(&conn, &new_job("other", 0), 1),
            Err(JobQueueError::Full { limit: 1 })
        ));
    }

    #[test]
    fn failed_jobs_store_bounded_diagnostics() {
        let conn = db();
        enqueue(&conn, &new_job("job", 0), 1).unwrap();
        claim_next(&conn).unwrap();
        fail(&conn, "job", &"x".repeat(700)).unwrap();
        let job = load(&conn, "job").unwrap().unwrap();
        assert_eq!(job.state, JobState::Failed);
        assert_eq!(job.error.unwrap().len(), 500);
        assert_eq!(job.error_kind.as_deref(), Some("unknown"));
        assert!(matches!(
            complete(&conn, "job"),
            Err(JobQueueError::NotRunning)
        ));
    }

    #[test]
    fn unchanged_failed_source_waits_for_a_new_fingerprint() {
        let conn = db();
        schedule_source(&conn, &new_job("src", 0), 10).unwrap();
        claim_next(&conn).unwrap();
        fail(&conn, "src", "source ingest failed").unwrap();

        assert_eq!(
            schedule_source(&conn, &new_job("src", 0), 10).unwrap(),
            SourceSchedule::CoalescedFailed
        );
        assert_eq!(load(&conn, "src").unwrap().unwrap().state, JobState::Failed);

        let changed = NewJob {
            payload_json: Some(r#"{"source":"changed"}"#),
            ..new_job("src", 1)
        };
        assert_eq!(
            schedule_source(&conn, &changed, 10).unwrap(),
            SourceSchedule::Enqueued
        );
    }

    #[test]
    fn schedule_source_coalesces_a_pending_storm() {
        let conn = db();
        assert_eq!(
            schedule_source(&conn, &new_job("src", 0), 10).unwrap(),
            SourceSchedule::Enqueued
        );
        // Repeated events for the same source collapse onto the pending job.
        assert_eq!(
            schedule_source(&conn, &new_job("src", 0), 10).unwrap(),
            SourceSchedule::CoalescedPending
        );
        let active: i64 = conn
            .query_row(
                "SELECT count(*) FROM job WHERE state = 'pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 1, "a storm must not inflate the queue");
    }

    #[test]
    fn change_during_run_is_not_lost_and_reruns() {
        let conn = db();
        schedule_source(&conn, &new_job("src", 0), 10).unwrap();
        let claimed = claim_next(&conn).unwrap().unwrap();
        assert_eq!(claimed.id, "src");

        // A change arrives while the job is running: flag a redo, do not lose it.
        assert_eq!(
            schedule_source(&conn, &new_job("src", 0), 10).unwrap(),
            SourceSchedule::MarkedRedo
        );

        // Finishing a redo-flagged job returns it to pending for another pass.
        assert_eq!(finish(&conn, "src").unwrap(), FinishOutcome::Requeued);
        assert_eq!(
            load(&conn, "src").unwrap().unwrap().state,
            JobState::Pending
        );

        // The re-run claims and finishes cleanly (no further redo pending).
        assert_eq!(claim_next(&conn).unwrap().unwrap().id, "src");
        assert_eq!(finish(&conn, "src").unwrap(), FinishOutcome::Done);
        assert_eq!(load(&conn, "src").unwrap().unwrap().state, JobState::Done);
    }

    #[test]
    fn finished_source_job_only_rearms_on_new_fingerprint() {
        let conn = db();
        schedule_source(&conn, &new_job("src", 0), 10).unwrap();
        claim_next(&conn).unwrap();
        assert_eq!(finish(&conn, "src").unwrap(), FinishOutcome::Done);

        assert_eq!(
            schedule_source(&conn, &new_job("src", 0), 10).unwrap(),
            SourceSchedule::CoalescedDone
        );

        // A later change re-arms the completed job rather than being dropped.
        let changed = NewJob {
            payload_json: Some(r#"{"source":"changed"}"#),
            ..new_job("src", 1)
        };
        assert_eq!(
            schedule_source(&conn, &changed, 10).unwrap(),
            SourceSchedule::Enqueued
        );
        assert_eq!(
            load(&conn, "src").unwrap().unwrap().state,
            JobState::Pending
        );
    }

    #[test]
    fn finish_requires_a_running_job() {
        let conn = db();
        schedule_source(&conn, &new_job("src", 0), 10).unwrap();
        // Pending, not running.
        assert!(matches!(
            finish(&conn, "src"),
            Err(JobQueueError::NotRunning)
        ));
    }

    #[test]
    fn queue_depth_reports_pending_and_running_counts() {
        let conn = db();
        assert_eq!(queue_depth(&conn).unwrap(), QueueDepth::default());
        enqueue(&conn, &new_job("a", 0), 10).unwrap();
        enqueue(&conn, &new_job("b", 0), 10).unwrap();
        assert_eq!(
            queue_depth(&conn).unwrap(),
            QueueDepth {
                pending: 2,
                running: 0
            }
        );
        claim_next(&conn).unwrap();
        let depth = queue_depth(&conn).unwrap();
        assert_eq!(depth.pending, 1);
        assert_eq!(depth.running, 1);
        assert_eq!(
            depth.active(),
            2,
            "active counts toward the capacity ceiling"
        );
        // Completed history does not inflate observed depth.
        complete(&conn, "a").unwrap();
        assert_eq!(queue_depth(&conn).unwrap().active(), 1);
    }

    #[test]
    fn restart_requeues_only_running_jobs() {
        let conn = db();
        enqueue(&conn, &new_job("running", 1), 2).unwrap();
        enqueue(&conn, &new_job("pending", 0), 2).unwrap();
        assert_eq!(claim_next(&conn).unwrap().unwrap().id, "running");

        assert_eq!(recover_running(&conn).unwrap(), 1);
        assert_eq!(
            load(&conn, "running").unwrap().unwrap().state,
            JobState::Pending
        );
        assert_eq!(
            load(&conn, "pending").unwrap().unwrap().state,
            JobState::Pending
        );
        let retried = claim_next(&conn).unwrap().unwrap();
        assert_eq!(retried.id, "running");
        assert_eq!(retried.attempts, 2);
    }

    #[test]
    fn recover_running_fails_poisoned_jobs_exceeding_max_attempts() {
        let conn = db();
        enqueue(&conn, &new_job("poisoned", 1), 2).unwrap();
        // Manually set attempts to MAX_JOB_ATTEMPTS and state to running.
        conn.execute(
            "UPDATE job SET state = 'running', attempts = ?1 WHERE id = 'poisoned'",
            [MAX_JOB_ATTEMPTS],
        )
        .unwrap();

        // Recovery should fail the poisoned job and return 0 requeued jobs.
        assert_eq!(recover_running(&conn).unwrap(), 0);
        let job = load(&conn, "poisoned").unwrap().unwrap();
        assert_eq!(job.state, JobState::Failed);
        assert_eq!(job.error_kind.as_deref(), Some("poisoned"));
    }
}
