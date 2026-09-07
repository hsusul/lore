//! `lorectl hook <EVENT>` — the entry point an agent calls, not a person.
//!
//! ## The rule that shapes everything here
//!
//! A hook runs inside someone's coding session. If it is slow, the session
//! stalls; if it fails, the agent may refuse to start. Neither is a price worth
//! paying for a status line. So this command is **read-only, bounded, and
//! always succeeds**: every failure path — no archive, not a repository, an
//! unreadable database, an event it has never heard of — exits 0 in silence.
//!
//! That is a deliberate inversion of the rest of the CLI, where a wrong
//! invocation is a usage error. Here the caller is a machine that cannot act on
//! the distinction, and a broken session is a far worse outcome than a missing
//! notice.
//!
//! ## What it says, and when it says nothing
//!
//! `session-start` reports agent-created work that is in the repository but on
//! no branch — the one landing outcome that is both provable and actionable
//! (`lore_core::landing`). When there is nothing at risk it prints nothing at
//! all. A hook that speaks every time is one people mute, and muting it is
//! worse than never having installed it.
//!
//! It does **not** scan. A scan is unbounded — it can walk hundreds of megabytes
//! — and taking that time at session start would make Lore the reason a session
//! is slow to begin. Keeping the archive current is the desktop app's job, or an
//! explicit `lorectl scan`.

use std::io::Write;

use lore_core::enrich::{resolve_repository, RepoResolution};
use lore_core::landing::{self, Landing, LandingIndex};
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::storage;

use crate::cli::Invocation;
use crate::exit::{self, CliError};

/// The event names this build understands.
///
/// An unknown event is not an error: agents add hook points over time, and a
/// Lore that refused an unfamiliar one would break sessions on upgrade.
const SESSION_START: &str = "session-start";

/// Run a hook event. Always returns `Ok(exit::OK)`.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let event = invocation.operand.as_deref().unwrap_or_default();
    if event == SESSION_START {
        // Errors are swallowed on purpose: see the module docs.
        let _ = session_start(invocation);
    }
    Ok(exit::OK)
}

/// Report at-risk agent work for the current repository, if any.
fn session_start(invocation: &Invocation) -> Result<(), CliError> {
    let cwd = std::env::current_dir().map_err(|_| CliError::NotARepository)?;
    let archive_dir = invocation.archive_dir()?;
    let conn = storage::open_read_only(&archive_dir.join(ARCHIVE_DB_FILENAME))?;

    let RepoResolution::InArchive { repository_id, .. } = resolve_repository(&conn, &cwd)? else {
        // Not a repository, or one Lore has nothing for. Nothing to say.
        return Ok(());
    };

    let at_risk = count_at_risk(&conn, &repository_id)?;
    if at_risk == 0 {
        return Ok(());
    }

    // One line, on stdout, phrased so an agent reading it knows what it is
    // looking at and a person skimming it knows what to do.
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(
        stdout,
        "lore: {at_risk} file(s) written by agents are staged and not committed \
         (a reset would discard them) — see `lorectl status`"
    );
    Ok(())
}

/// How many recorded changes are present in the repository but unreferenced.
///
/// Walks the repository's history once; see `lore_core::landing`.
fn count_at_risk(conn: &rusqlite::Connection, repository_id: &str) -> Result<usize, CliError> {
    let mut stmt = conn
        .prepare(
            "SELECT f.content_oid FROM file_event f
             JOIN session_segment sg ON sg.id = f.segment_id
             WHERE sg.repository_id = ?1 AND f.content_oid IS NOT NULL",
        )
        .map_err(lore_core::storage::StorageError::from)?;
    let oids: Vec<String> = stmt
        .query_map([repository_id], |row| row.get(0))
        .map_err(lore_core::storage::StorageError::from)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(lore_core::storage::StorageError::from)?;
    if oids.is_empty() {
        return Ok(0);
    }

    let Some((worktree, index)) = landing::live_worktrees(conn, repository_id)?
        .into_iter()
        .find_map(|w| LandingIndex::build(&w).map(|i| (w, i)))
    else {
        return Ok(0);
    };
    Ok(oids
        .iter()
        // Staged only. `Unreferenced` content is not discarded by a reset, so
        // announcing it under the same warning would be false.
        .filter(|oid| matches!(landing::classify(&worktree, &index, oid), Landing::Staged))
        .count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_event_is_silently_accepted() {
        // Agents gain hook points over time. Refusing an unfamiliar one would
        // break sessions the day the agent adds it.
        let invocation = crate::cli::parse(["hook", "some-future-event"]).unwrap();
        assert_eq!(run(&invocation).unwrap(), exit::OK);
    }

    #[test]
    fn a_missing_archive_does_not_fail_the_session() {
        // The failure most likely to happen in the wild — Lore not set up yet —
        // must be invisible to the agent.
        let invocation =
            crate::cli::parse(["--archive", "/nonexistent/archive", "hook", SESSION_START])
                .unwrap();
        assert_eq!(run(&invocation).unwrap(), exit::OK);
    }

    #[test]
    fn a_relative_archive_does_not_fail_the_session() {
        // Everywhere else in the CLI this is a usage error. Inside a hook it is
        // a silent no-op, because the caller is a machine that cannot act on it.
        let invocation =
            crate::cli::parse(["--archive", "relative/path", "hook", SESSION_START]).unwrap();
        assert_eq!(run(&invocation).unwrap(), exit::OK);
    }
}
