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
//! - whether recorded file content reached a commit, where the content is
//!   genuinely known (`lore_core::landing`).
//!
//! The landing ladder is discrete and every rung is an observation, never a
//! verdict: *in a commit*, *in the repository but not on any branch*, *no
//! observed landing*, *not assessed*. The last two are different in kind — one
//! means Lore looked and did not find it, the other that Lore had nothing to
//! look for, because an `edit` records a diff rather than the resulting file.
//! Words like *lost*, *abandoned*, *uncommitted*, *unfinished*, and *never
//! landed* remain forbidden: they assert something about the developer's work,
//! and Lore only ever knows what it looked for and found.
//!
//! Every count is therefore reported next to **when Lore last looked**. A
//! session count without that is uninterpretable: "no archived sessions" and
//! "no scan has ever run" look identical to a reader, and only one of them says
//! anything about the work.

use std::io::Write;

use lore_core::enrich::{resolve_repository, RepoResolution};
use lore_core::landing::{self, Landing, LandingIndex};
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::{settings, storage};

use crate::cli::Invocation;
use crate::exit::{self, CliError};
use crate::scan::now_ms;

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

    let last_scan: Option<i64> = settings::last_scan_completed_at(&conn)?;

    let (repository_id, display_name) = match &resolution {
        RepoResolution::InArchive {
            repository_id,
            display_name,
            ..
        } => (Some(repository_id.clone()), display_name.clone()),
        RepoResolution::NotInArchive { display_name, .. } => (None, display_name.clone()),
        RepoResolution::NotARepository => unreachable!("handled above"),
    };

    let total = match &repository_id {
        Some(id) => repository_session_count(&conn, id)?,
        None => 0,
    };

    let report = match &repository_id {
        Some(id) => risk_report(&conn, id)?,
        None => RiskReport::default(),
    };

    let mut stdout = std::io::stdout().lock();
    if invocation.json {
        let line = serde_json::json!({
            "repository": display_name,
            "repository_id": repository_id,
            "in_archive": repository_id.is_some(),
            "sessions": total,
            "last_scan_completed_at_ms": last_scan,
            // The finding, first: content that exists in this repository but is
            // on no branch, and would be discarded by an ordinary reset.
            "at_risk": report.at_risk.iter().map(|c| serde_json::json!({
                "session_id": c.session_id,
                "agent_id": c.agent_id,
                "title": c.title,
                "path": c.path,
            })).collect::<Vec<_>>(),
            "at_risk_count": report.at_risk.len(),
            // Reported, but never as risk — see `RiskReport::unseen`.
            "not_found_in_this_repository": report.unseen,
            "in_a_commit": report.committed,
            "not_assessed": report.not_assessed,
            "assesses": ["repository", "sessions", "last_scan", "landing"],
        });
        let _ = writeln!(stdout, "{line}");
        return Ok(exit::OK);
    }

    // The finding first, before any inventory. If there is something to act on,
    // it should be the first thing on screen.
    if !report.at_risk.is_empty() {
        let _ = writeln!(
            stdout,
            "AT RISK  {} file(s) created by agents are in this repository but on no branch",
            report.at_risk.len()
        );
        let _ = writeln!(stdout, "         a reset or clean would discard them\n");
        let mut current = String::new();
        for change in &report.at_risk {
            if change.session_id != current {
                current.clone_from(&change.session_id);
                let title = change.title.as_deref().unwrap_or("(untitled)");
                let _ = writeln!(
                    stdout,
                    "  {}  {}  {title}",
                    change.session_id, change.agent_id
                );
            }
            let _ = writeln!(stdout, "      {}", short_path(&change.path));
        }
        let _ = writeln!(stdout);
    } else if report.committed > 0 {
        // Said positively, and only when something was actually checked.
        let _ = writeln!(
            stdout,
            "nothing at risk — {} agent change(s) are in commits\n",
            report.committed
        );
    }

    let _ = writeln!(stdout, "repository  {display_name}");
    match repository_id {
        Some(_) => {
            let _ = writeln!(stdout, "sessions    {total} archived for this repository");
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

    if report.unseen > 0 {
        // Deliberately not called risk: a file the agent created and someone
        // then edited before committing is indistinguishable from one that was
        // never committed, and Lore cannot tell them apart.
        let _ = writeln!(
            stdout,
            "unseen      {} recorded change(s) not found in this repository \
             (may have been edited before committing)",
            report.unseen
        );
    }
    if report.assessed() == 0 && report.not_assessed > 0 {
        // The honest headline when nothing could be checked at all.
        let _ = writeln!(
            stdout,
            "note        no change could be checked — an edit records a change, \
             not the resulting file"
        );
    }
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

/// Agent-created content that is **in the repository's object database but on
/// no branch** — an index entry, or a commit that was rewritten away.
///
/// This is the one landing outcome that is both provable and actionable: the
/// bytes exist, nothing references them, and an ordinary `git reset` or `git
/// clean` discards them for good. It is the finding `status` leads with.
#[derive(Debug, Clone)]
struct AtRiskChange {
    session_id: String,
    agent_id: String,
    title: Option<String>,
    path: String,
}

/// What `status` found for one repository.
#[derive(Debug, Default, Clone)]
struct RiskReport {
    /// Provably at risk: present, unreferenced.
    at_risk: Vec<AtRiskChange>,
    /// Content Lore recorded and cannot find in this repository at all.
    ///
    /// Reported **separately and softly**, never as risk: a file the agent
    /// created and someone then edited before committing looks exactly like a
    /// file that was never committed. Lore cannot tell those apart, so it does
    /// not pretend to.
    unseen: usize,
    /// Confirmed in a commit.
    committed: usize,
    /// No recorded content to check — an `edit` records a change, not the
    /// resulting file.
    not_assessed: usize,
}

impl RiskReport {
    fn assessed(&self) -> usize {
        self.at_risk.len() + self.unseen + self.committed
    }
}

/// Classify every recorded change for a repository, keeping the identity of the
/// ones that are at risk so the report can name them.
///
/// The repository's object history is walked **once** into a [`LandingIndex`];
/// resolving each change independently would re-walk history per change.
fn risk_report(conn: &rusqlite::Connection, repository_id: &str) -> Result<RiskReport, CliError> {
    let mut stmt = conn
        .prepare(
            "SELECT f.content_oid, f.path, s.id, s.agent_id, s.title
             FROM file_event f
             JOIN session_segment sg ON sg.id = f.segment_id
             JOIN agent_session s ON s.id = f.session_id
             WHERE sg.repository_id = ?1
             ORDER BY s.started_at DESC",
        )
        .map_err(lore_core::storage::StorageError::from)?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Option<String>, String, String, String, Option<String>)> = stmt
        .query_map([repository_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(lore_core::storage::StorageError::from)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(lore_core::storage::StorageError::from)?;

    let mut report = RiskReport::default();
    let worktrees = landing::live_worktrees(conn, repository_id)?;
    let index = worktrees
        .iter()
        .find_map(|w| LandingIndex::build(w).map(|i| (w.clone(), i)));

    for (oid, path, session_id, agent_id, title) in rows {
        let Some(oid) = oid else {
            report.not_assessed += 1;
            continue;
        };
        let Some((worktree, index)) = &index else {
            // An oid but nowhere to look. Not a finding either way.
            report.unseen += 1;
            continue;
        };
        match landing::classify(worktree, index, &oid) {
            Landing::Committed => report.committed += 1,
            Landing::Staged => report.at_risk.push(AtRiskChange {
                session_id,
                agent_id,
                title,
                path,
            }),
            Landing::NoObservedLanding | Landing::NotAssessed => report.unseen += 1,
        }
    }
    Ok(report)
}

/// The last path segments of `path`, so a long absolute path stays readable
/// without hiding which file it is.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.rsplit('/').take(3).collect();
    let tail = parts.into_iter().rev().collect::<Vec<_>>().join("/");
    if tail.len() < path.len() {
        format!("…/{tail}")
    } else {
        tail
    }
}
