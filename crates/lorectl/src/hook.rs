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
//! After that it lists the accepted memory notes that apply where the session
//! runs, each with its freshness label, so an agent starts knowing them. The
//! directory is the `cwd` of the hook JSON on stdin, else the process's own.
//! No vault, no note in scope, or any vault error adds nothing. Recall may
//! refresh the vault's derived index under `.lore/`, the same cache
//! `lorectl mem recall` keeps, and waits at most `LOCK_WAIT` for it.
//!
//! It does **not** scan. A scan is unbounded — it can walk hundreds of megabytes
//! — and taking that time at session start would make Lore the reason a session
//! is slow to begin. Keeping the archive current is the desktop app's job, or an
//! explicit `lorectl scan`.

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use lore_core::enrich::{resolve_repository, RepoResolution};
use lore_core::landing::{self, Landing, LandingIndex};
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::storage;
use lore_memory::index::Index;
use lore_memory::vault::{self, Vault};
use lore_memory::{inbox, recall};

use crate::cli::Invocation;
use crate::exit::{self, CliError};

/// The event names this build understands.
///
/// An unknown event is not an error: agents add hook points over time, and a
/// Lore that refused an unfamiliar one would break sessions on upgrade.
const SESSION_START: &str = "session-start";

/// At most this many notes at session start. Experiment Q3 saw agents follow
/// them at 5, 10, and 25; the cap is set by what the context costs.
const NOTE_CAP: usize = 10;
/// The notes section stops adding note lines past this many bytes.
const CHAR_CAP: usize = 2_000;
/// Hook JSON past this many bytes is not read.
const STDIN_CAP: u64 = 1 << 20;

/// Run a hook event. Always returns `Ok(exit::OK)`.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let event = invocation.operand.as_deref().unwrap_or_default();
    if event == SESSION_START {
        let cwd = hook_cwd();
        // Errors are swallowed on purpose: see the module docs.
        let _ = session_start(invocation);
        if let Some(block) = memory_notes(cwd) {
            let _ = std::io::stdout().lock().write_all(block.as_bytes());
        }
    }
    Ok(exit::OK)
}

/// The absolute `cwd` an agent passes in its hook JSON on stdin, if any. A
/// terminal is never read, so a person running the hook is not left waiting.
fn hook_cwd() -> Option<PathBuf> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut text = String::new();
    stdin
        .lock()
        .take(STDIN_CAP)
        .read_to_string(&mut text)
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let cwd = PathBuf::from(json.get("cwd")?.as_str()?);
    cwd.is_absolute().then_some(cwd)
}

/// The notes section for `cwd`, or `None` when there is nothing to add.
fn memory_notes(cwd: Option<PathBuf>) -> Option<String> {
    let cwd = cwd.or_else(|| std::env::current_dir().ok())?;
    let vault = Vault::open(&vault::resolve_dir(None).ok()?).ok()?;
    let found = recall::recall(&vault, "", &cwd, NOTE_CAP).ok()?;
    if found.notes.is_empty() {
        return None;
    }
    // Recall left the index fresh, so counting what it capped is one query.
    let scope = found.context.key().map(|k| format!("repo:{}", k.as_str()));
    let total = Index::open(&vault.lore_dir())
        .ok()
        .flatten()
        .and_then(|index| index.list_active(scope.as_deref(), usize::MAX).ok())
        .map_or(0, |all| all.len());
    let lines = found.notes.iter().map(|recalled| {
        format!(
            "- [{}] {} ({})",
            recalled.freshness.label(),
            title(&recalled.note.title),
            recalled.note.id.as_str()
        )
    });
    Some(render(lines, total.max(found.notes.len())))
}

/// A note title fit for an agent's context: secrets masked, on one line.
fn title(text: &str) -> String {
    inbox::redact(text)
        .unwrap_or_else(|_| "(title withheld)".to_owned())
        .replace(char::is_control, " ")
}

/// A heading, one line per note while the section stays under [`CHAR_CAP`],
/// then a line saying how many of `total` were left out and where they are.
fn render(lines: impl IntoIterator<Item = String>, total: usize) -> String {
    let mut out = String::from("Lore memory for this repo:\n");
    let mut shown = 0;
    for line in lines {
        if out.len() + line.len() + 1 > CHAR_CAP {
            break;
        }
        out.push_str(&line);
        out.push('\n');
        shown += 1;
    }
    let left = total.saturating_sub(shown);
    if left > 0 {
        out.push_str(&format!(
            "- {left} more not shown. The recall_memory tool lists them.\n"
        ));
    }
    out
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
        // `run` swallows this error. `run` itself is tested end to end in
        // tests/hook.rs, where stdin and the vault are the test's own.
        assert!(session_start(&invocation).is_err());
    }

    #[test]
    fn a_relative_archive_does_not_fail_the_session() {
        // Everywhere else in the CLI this is a usage error. Inside a hook it is
        // a silent no-op, because the caller is a machine that cannot act on it.
        let invocation =
            crate::cli::parse(["--archive", "relative/path", "hook", SESSION_START]).unwrap();
        assert!(session_start(&invocation).is_err());
    }

    #[test]
    fn the_notes_section_stops_near_two_thousand_bytes() {
        let line = format!("- [fresh] {} (01JD0000000000000000000001)", "t".repeat(400));
        let out = render(std::iter::repeat_n(line.clone(), 10), 10);
        let expected = format!(
            "Lore memory for this repo:\n{line}\n{line}\n{line}\n{line}\n\
             - 6 more not shown. The recall_memory tool lists them.\n"
        );
        assert_eq!(out, expected);
        assert_eq!(
            render(["- [gone] a (b)".to_owned()], 1),
            "Lore memory for this repo:\n- [gone] a (b)\n"
        );
    }

    #[test]
    fn a_title_is_redacted_and_kept_on_one_line() {
        assert_eq!(title("two\nlines"), "two lines");
        let key = concat!("AKIA", "Z7Q4R2T6Y8U1W3E5");
        let shown = title(&format!("rotate {key}"));
        assert!(
            shown.starts_with("rotate ") && !shown.contains(key),
            "{shown}"
        );
    }
}
