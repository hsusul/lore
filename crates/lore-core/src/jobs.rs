//! Durable bounded job queue with backpressure.
//!
//! The queue is persisted in SQLite so process restarts can return interrupted
//! work to `pending`. Claiming is transactional and respects priority then FIFO.
//! Worker threads and progress events are app-layer concerns added later in M3.

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
                        error = NULL, updated_at = unixepoch('now')*1000
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
    transition_running(conn, id, "done", None)
}

/// Mark a claimed task failed with a bounded, caller-scrubbed diagnostic.
pub fn fail(conn: &Connection, id: &str, error: &str) -> Result<()> {
    let bounded: String = error.chars().take(500).collect();
    transition_running(conn, id, "failed", Some(&bounded))
}

/// Return tasks left `running` by a terminated process to `pending`.
pub fn recover_running(conn: &Connection) -> Result<usize> {
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
    error: Option<&str>,
) -> Result<()> {
    let changed = conn.execute(
        "UPDATE job SET state = ?2, error = ?3, updated_at = unixepoch('now')*1000
         WHERE id = ?1 AND state = 'running'",
        params![id, target, error],
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(JobQueueError::NotRunning)
    }
}

fn load_job(conn: &Connection, id: &str) -> Result<Option<Job>> {
    conn.query_row(
        "SELECT id, kind, priority, state, attempts, payload_json, error, created_at, updated_at
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
            ))
        },
    )
    .optional()?
    .map(
        |(id, kind, priority, state, attempts, payload_json, error, created_at, updated_at)| {
            Ok(Job {
                id,
                kind,
                priority,
                state: JobState::parse(&state)?,
                attempts,
                payload_json,
                error,
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
        assert!(matches!(
            complete(&conn, "job"),
            Err(JobQueueError::NotRunning)
        ));
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
}
