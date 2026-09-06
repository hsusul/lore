//! `lorectl status` — what Lore has recorded about the repository you are in.
//!
//! ## What this command may and may not say
//!
//! The pitch for `status` is "`git status` for work coding agents performed".
//! The honest version of that, today, is narrower than the pitch, and this
//! module is deliberately built to the honest version.
//!
//! Lore can currently prove:
//! - which repository this directory is (`enrich::resolve_repository`);
//! - which archived sessions have segments linked to that repository;
//! - when a scan of this archive last *completed* (`scan.last_completed_at`).
//!
//! Lore cannot yet prove anything about **landing** — whether the work in those
//! sessions reached a commit. That needs per-change content identity and a
//! commit path, which do not exist yet. So this command says nothing about it,
//! rather than implying it with a silence a reader would fill in. Words like
//! *lost*, *abandoned*, *uncommitted*, *unfinished*, and *never landed* are
//! forbidden here until they are earned.
//!
//! Every count is therefore reported next to **when Lore last looked**. A
//! session count without that is uninterpretable: "no archived sessions" and
//! "no scan has ever run" look identical to a reader, and only one of them says
//! anything about the work.

use std::io::Write;

use lore_core::enrich::{resolve_repository, RepoResolution};
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::{query, settings, storage};

use crate::cli::Invocation;
use crate::exit::{self, CliError};
use crate::scan::KEY_LAST_SCAN_COMPLETED_AT;

/// How many recent sessions to name. The count is always exact; this bounds
/// only the listing.
const RECENT_LIMIT: i64 = 5;
const _: () = assert!(RECENT_LIMIT > 0);

/// `lorectl status` — archived agent work for the current repository.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|_| CliError::Usage("could not determine the current directory".into()))?;

    let archive_dir = invocation.archive_dir()?;
    let conn = storage::open_read_only(&archive_dir.join(ARCHIVE_DB_FILENAME))?;

    let resolution = resolve_repository(&conn, &cwd)?;
    if resolution == RepoResolution::NotARepository {
        // The one condition exit 3 was reserved for. Not an archive problem:
        // the archive may be perfectly fine and simply not the issue.
        return Err(CliError::NotARepository);
    }

    let last_scan: Option<i64> =
        settings::get(&conn, KEY_LAST_SCAN_COMPLETED_AT)?.and_then(|raw| raw.parse::<i64>().ok());

    let (repository_id, display_name) = match &resolution {
        RepoResolution::InArchive {
            repository_id,
            display_name,
            ..
        } => (Some(repository_id.clone()), display_name.clone()),
        RepoResolution::NotInArchive { display_name, .. } => (None, display_name.clone()),
        RepoResolution::NotARepository => unreachable!("handled above"),
    };

    let sessions = match &repository_id {
        Some(id) => query::list_repository_sessions(&conn, id, RECENT_LIMIT)?,
        None => Vec::new(),
    };
    let total = match &repository_id {
        Some(id) => repository_session_count(&conn, id)?,
        None => 0,
    };

    let mut stdout = std::io::stdout().lock();
    if invocation.json {
        let line = serde_json::json!({
            "repository": display_name,
            "repository_id": repository_id,
            "in_archive": repository_id.is_some(),
            "sessions": total,
            "last_scan_completed_at_ms": last_scan,
            "recent": sessions.iter().map(|s| serde_json::json!({
                "session_id": s.id,
                "agent_id": s.agent_id,
                "title": s.title,
                "started_at_ms": s.started_at,
            })).collect::<Vec<_>>(),
            // Named explicitly so a machine reader cannot mistake the absence of
            // a landing field for a claim that nothing landed.
            "landing": serde_json::Value::Null,
            "landing_note": "not yet assessed by this build",
        });
        let _ = writeln!(stdout, "{line}");
        return Ok(exit::OK);
    }

    let _ = writeln!(stdout, "repository  {display_name}");
    match repository_id {
        Some(_) => {
            let _ = writeln!(stdout, "sessions    {total} archived for this repository");
            for session in &sessions {
                let title = session.title.as_deref().unwrap_or("(untitled)");
                let _ = writeln!(stdout, "  {}  {}  {title}", session.id, session.agent_id);
            }
            if total > i64::try_from(sessions.len()).unwrap_or(i64::MAX) {
                let _ = writeln!(stdout, "  … and {} more", total - sessions.len() as i64);
            }
        }
        None => {
            // Phrased as a fact about the archive, not about the work.
            let _ = writeln!(
                stdout,
                "sessions    none recorded in this archive for this repository"
            );
        }
    }
    let _ = writeln!(stdout, "last scan   {}", describe_last_scan(last_scan));
    // The limit of what this build knows, stated rather than left to inference.
    let _ = writeln!(
        stdout,
        "note        whether this work landed in Git is not assessed by this build"
    );
    Ok(exit::OK)
}

/// Exact number of archived sessions with a segment in this repository.
///
/// Counted rather than inferred from the bounded listing: the listing is capped
/// for readability, and reporting its length as the total would understate the
/// archive the moment a repository had more than a handful of sessions.
fn repository_session_count(
    conn: &rusqlite::Connection,
    repository_id: &str,
) -> Result<i64, CliError> {
    let count = conn
        .query_row(
            "SELECT count(DISTINCT sg.session_id) FROM session_segment sg
             WHERE sg.repository_id = ?1",
            [repository_id],
            |row| row.get(0),
        )
        .map_err(lore_core::storage::StorageError::from)?;
    Ok(count)
}

/// Describe when Lore last completed a scan.
///
/// "never" is a distinct, load-bearing answer: a zero session count from an
/// archive that has never been scanned says nothing at all about the work, and a
/// reader must be able to tell that case apart.
fn describe_last_scan(last_scan_ms: Option<i64>) -> String {
    let Some(at) = last_scan_ms else {
        return "never — run `lorectl scan`".to_string();
    };
    let now = now_ms();
    // A stamp from the future means a clock changed, not that the future
    // happened; report it plainly instead of rendering a negative age.
    if at > now {
        return format!("{at} (clock skew: stamp is ahead of this machine)");
    }
    format!("{} ago", humanize_ms(now - at))
}

/// Coarse, dependency-free duration wording. Deliberately imprecise: the point
/// is "recently" versus "a long time ago", and a false-precise "3.7 hours" would
/// invite more confidence than a scan stamp deserves.
fn humanize_ms(delta: i64) -> String {
    const SECOND: i64 = 1_000;
    const MINUTE: i64 = 60 * SECOND;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;

    let plural = |n: i64, unit: &str| {
        if n == 1 {
            format!("1 {unit}")
        } else {
            format!("{n} {unit}s")
        }
    };
    match delta {
        d if d < MINUTE => "less than a minute".to_string(),
        d if d < HOUR => plural(d / MINUTE, "minute"),
        d if d < DAY => plural(d / HOUR, "hour"),
        d => plural(d / DAY, "day"),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_never_scanned_archive_says_so_and_names_the_fix() {
        // The most important wording in this module: without it, "0 sessions"
        // from an unscanned archive reads as "no work happened here".
        let text = describe_last_scan(None);
        assert!(text.contains("never"), "{text}");
        assert!(text.contains("lorectl scan"), "{text}");
    }

    #[test]
    fn a_recent_scan_reads_as_recent() {
        let now = now_ms();
        assert!(describe_last_scan(Some(now - 30_000)).contains("less than a minute"));
        assert!(describe_last_scan(Some(now - 5 * 60_000)).contains("5 minutes ago"));
        assert!(describe_last_scan(Some(now - 2 * 3_600_000)).contains("2 hours ago"));
        assert!(describe_last_scan(Some(now - 3 * 86_400_000)).contains("3 days ago"));
    }

    #[test]
    fn a_stamp_from_the_future_is_reported_as_skew_not_a_negative_age() {
        let text = describe_last_scan(Some(now_ms() + 60_000));
        assert!(text.contains("skew"), "{text}");
        assert!(!text.contains('-'), "rendered a negative age: {text}");
    }

    #[test]
    fn durations_are_singular_at_one() {
        assert_eq!(humanize_ms(60_000), "1 minute");
        assert_eq!(humanize_ms(3_600_000), "1 hour");
        assert_eq!(humanize_ms(86_400_000), "1 day");
        assert_eq!(humanize_ms(120_000), "2 minutes");
    }

    #[test]
    fn durations_never_claim_more_precision_than_they_have() {
        // Rounds down to a whole unit; 90 minutes is "1 hour", not "1.5 hours".
        assert_eq!(humanize_ms(90 * 60_000), "1 hour");
        assert_eq!(humanize_ms(0), "less than a minute");
    }
}
