//! Progress through the most recent days of history, one entry per day, for
//! a first run that reads the newest days first. A day's sessions are counted
//! by when they were last active, the same instant an ingest job is ordered
//! by (the source's mtime), so `read` and `waiting` describe one day.

use rusqlite::{params, Connection};

use crate::storage::Result;

const DAY_MS: i64 = 86_400_000;

/// One day: sessions already in the archive, and sources still queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayProgress {
    /// The day's local midnight, as epoch milliseconds.
    pub day_start_ms: i64,
    /// Sessions in the archive last active that day.
    pub read: i64,
    /// Ingest jobs still pending or running for sources modified that day.
    pub waiting: i64,
}

/// The last `days` days up to and including the one holding `now_ms`, oldest
/// first. Days are local to `utc_offset_ms` (east of UTC is positive).
pub fn recent_days(
    conn: &Connection,
    now_ms: i64,
    utc_offset_ms: i64,
    days: i64,
) -> Result<Vec<DayProgress>> {
    let today = (now_ms + utc_offset_ms).div_euclid(DAY_MS);
    let first = today - days.max(1) + 1;
    let mut out: Vec<DayProgress> = (first..=today)
        .map(|day| DayProgress {
            day_start_ms: day * DAY_MS - utc_offset_ms,
            read: 0,
            waiting: 0,
        })
        .collect();
    let since_ms = first * DAY_MS - utc_offset_ms;
    let mut tally = |sql: &str, set: fn(&mut DayProgress, i64)| -> Result<()> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![utc_offset_ms, DAY_MS, since_ms], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (day, count) = row?;
            if let Some(entry) = usize::try_from(day - first)
                .ok()
                .and_then(|i| out.get_mut(i))
            {
                set(entry, count);
            }
        }
        Ok(())
    };
    tally(
        "SELECT (COALESCE(ended_at, started_at) + ?1) / ?2, count(*) FROM agent_session
         WHERE COALESCE(ended_at, started_at) >= ?3 GROUP BY 1",
        |d, n| d.read = n,
    )?;
    tally(
        &format!(
            "SELECT (priority + ?1) / ?2, count(*) FROM job
             WHERE kind = '{}' AND state IN ('pending', 'running') AND priority >= ?3
             GROUP BY 1",
            crate::pipeline::JOB_KIND
        ),
        |d, n| d.waiting = n,
    )?;
    Ok(out)
}
