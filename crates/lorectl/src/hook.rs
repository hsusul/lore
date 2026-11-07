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
//! Both parts below look where the session runs: the `cwd` of the hook JSON on
//! stdin, else the process's own.
//!
//! `session-start` reports agent-created work that is in the repository but on
//! no branch — the one landing outcome that is both provable and actionable
//! (`lore_core::landing`). When there is nothing at risk it prints nothing at
//! all. A hook that speaks every time is one people mute, and muting it is
//! worse than never having installed it.
//!
//! After that it lists the accepted memory notes that apply where the session
//! runs, each with its freshness label, so an agent starts knowing them. No
//! vault, no note in scope, or any vault error adds nothing. Recall may
//! refresh the vault's derived index under `.lore/`, the same cache
//! `lorectl mem recall` keeps, and waits at most `LOCK_WAIT` for it.
//!
//! Last, when the hook JSON's `session_id` names a session the archive holds,
//! by its archive id or its agent's own id, it prints `Lore session: <id>`
//! with the archive id. An agent can pass that id to `propose_memory`, which
//! then records the proposal's source as verified. A session started since
//! the last scan is not in the archive yet, so it gets no line.
//!
//! `pre-edit` runs before a tool changes a file. It reads `tool_input.file_path`
//! (or `notebook_path`) from the hook JSON and prints the accepted notes
//! anchored to that file, each with its freshness label, as the PreToolUse
//! `additionalContext` reply Claude Code and Codex pass to the model. It
//! never answers with a permission decision, so it cannot allow or block the
//! edit. No note on that file prints nothing.
//!
//! It does **not** scan. A scan is unbounded — it can walk hundreds of megabytes
//! — and taking that time at session start would make Lore the reason a session
//! is slow to begin. Keeping the archive current is the desktop app's job, or an
//! explicit `lorectl scan`.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lore_core::enrich::{resolve_repository, RepoResolution};
use lore_core::landing;
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::{query, storage};
use lore_memory::note::RelPath;
use lore_memory::recall::{self, Recalled};
use lore_memory::repo::RepoContext;
use lore_memory::session_start::{self, title};
use lore_memory::vault::{self, Vault};

use crate::cli::Invocation;
use crate::exit::{self, CliError};

/// The event names this build understands.
///
/// An unknown event is not an error: agents add hook points over time, and a
/// Lore that refused an unfamiliar one would break sessions on upgrade.
const SESSION_START: &str = "session-start";
const PRE_EDIT: &str = "pre-edit";

/// At most this many notes before an edit, in at most this many bytes.
const EDIT_NOTE_CAP: usize = 5;
const EDIT_CHAR_CAP: usize = 1_000;
/// Hook JSON past this many bytes is not read.
const STDIN_CAP: u64 = 1 << 20;
/// The history walk behind the at-risk line gives up past this. Half the 10 s
/// the Claude Code plugin allows the hook, so the notes still get their turn.
const AT_RISK_BUDGET: Duration = Duration::from_secs(5);

/// Run a hook event. Always returns `Ok(exit::OK)`.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let event = invocation.operand.as_deref().unwrap_or_default();
    if event == SESSION_START {
        let input = hook_input();
        let cwd = input.cwd.or_else(|| std::env::current_dir().ok());
        // Errors are swallowed on purpose: see the module docs.
        let _ = session_start(invocation, cwd.as_deref());
        let mut stdout = std::io::stdout().lock();
        if let Some(block) = memory_notes(cwd.as_deref()) {
            let _ = stdout.write_all(block.as_bytes());
        }
        if let Some(id) = input.session_id.and_then(|id| archived(invocation, &id)) {
            let _ = writeln!(stdout, "Lore session: {id}");
        }
    } else if event == PRE_EDIT {
        if let Some(text) = anchored_notes(&hook_input()) {
            // Plain stdout before a tool call reaches no model; this field does.
            let reply = serde_json::json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse", "additionalContext": text } });
            let _ = writeln!(std::io::stdout().lock(), "{reply}");
        }
    }
    Ok(exit::OK)
}

/// What an agent's hook JSON on stdin says, as far as this hook uses it.
#[derive(Default)]
struct HookInput {
    /// The absolute `cwd`.
    cwd: Option<PathBuf>,
    session_id: Option<String>,
    /// The file a tool is about to change, as the agent wrote it.
    file_path: Option<PathBuf>,
}

/// The hook JSON on stdin. Anything unreadable reads as empty.
fn hook_input() -> HookInput {
    let json = stdin_json().unwrap_or_default();
    let text = |key: &str| json.get(key).and_then(serde_json::Value::as_str);
    HookInput {
        cwd: text("cwd")
            .map(PathBuf::from)
            .filter(|cwd| cwd.is_absolute()),
        session_id: text("session_id")
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
        file_path: ["/tool_input/file_path", "/tool_input/notebook_path"]
            .iter()
            .find_map(|at| json.pointer(at).and_then(serde_json::Value::as_str))
            .filter(|path| !path.is_empty())
            .map(PathBuf::from),
    }
}

/// Stdin parsed as JSON. A terminal is never read, so a person running the
/// hook is not left waiting.
fn stdin_json() -> Option<serde_json::Value> {
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
    serde_json::from_str(&text).ok()
}

/// The archive id of the session `id` names, read only. `None` when there is
/// no archive or it holds no such session.
fn archived(invocation: &Invocation, id: &str) -> Option<String> {
    let archive_dir = invocation.archive_dir().ok()?;
    let conn = storage::open_read_only(&archive_dir.join(ARCHIVE_DB_FILENAME)).ok()?;
    Some(query::find_session(&conn, id).ok()??.archive_id)
}

/// The notes section for `cwd`, or `None` when there is nothing to add.
fn memory_notes(cwd: Option<&Path>) -> Option<String> {
    let vault = Vault::open(&vault::resolve_dir(None).ok()?).ok()?;
    let found = session_start::session_start(&vault, cwd?).ok()??;
    Some(found.text)
}

/// The notes anchored to the file the hook JSON says a tool will change, or
/// `None` when there are none. A relative path is relative to `cwd`.
fn anchored_notes(input: &HookInput) -> Option<String> {
    let cwd = input.cwd.clone().or_else(|| std::env::current_dir().ok())?;
    let file = cwd.join(input.file_path.as_ref()?);
    let root = RepoContext::discover(&cwd).root()?.to_path_buf();
    let rel = repo_relative(&root, &file)?;
    let vault = Vault::open(&vault::resolve_dir(None).ok()?).ok()?;
    let found = recall::anchored_to(&vault, &cwd, &rel, EDIT_NOTE_CAP).ok()?;
    let mut out = format!("Lore notes anchored to {}:\n", rel.as_str());
    let heading = out.len();
    for line in found.iter().map(note_line) {
        if out.len() + line.len() + 1 > EDIT_CHAR_CAP {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    (out.len() > heading).then_some(out)
}

/// `file` relative to `root`, comparing real paths when the two are spelled
/// differently. The file itself may not exist yet; its folder must.
fn repo_relative(root: &Path, file: &Path) -> Option<RelPath> {
    let rel = match file.strip_prefix(root) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => {
            let dir = std::fs::canonicalize(file.parent()?).ok()?;
            let real = dir.join(file.file_name()?);
            real.strip_prefix(std::fs::canonicalize(root).ok()?)
                .ok()?
                .to_path_buf()
        }
    };
    RelPath::parse(&rel.to_str()?.replace('\\', "/")).ok()
}

/// One note as a line: its freshness label, its title, and its id.
fn note_line(recalled: &Recalled) -> String {
    format!(
        "- [{}] {} ({})",
        recalled.freshness.label(),
        title(&recalled.note.title),
        recalled.note.id.as_str()
    )
}

/// Report at-risk agent work for the current repository, if any.
fn session_start(invocation: &Invocation, cwd: Option<&Path>) -> Result<(), CliError> {
    let cwd = cwd.ok_or(CliError::NotARepository)?;
    let archive_dir = invocation.archive_dir()?;
    let conn = storage::open_read_only(&archive_dir.join(ARCHIVE_DB_FILENAME))?;

    let RepoResolution::InArchive { repository_id, .. } = resolve_repository(&conn, cwd)? else {
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
/// Walks history only when staged content is missing from `HEAD`, and then
/// within [`AT_RISK_BUDGET`]; see `lore_core::landing::count_staged`.
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

    // Staged only. `Unreferenced` content is not discarded by a reset, so
    // announcing it under the same warning would be false.
    let deadline = Instant::now() + AT_RISK_BUDGET;
    Ok(landing::live_worktrees(conn, repository_id)?
        .iter()
        .find_map(|w| landing::count_staged(w, &oids, deadline))
        .unwrap_or(0))
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
        assert!(session_start(&invocation, Some(Path::new("/"))).is_err());
    }

    #[test]
    fn a_relative_archive_does_not_fail_the_session() {
        // Everywhere else in the CLI this is a usage error. Inside a hook it is
        // a silent no-op, because the caller is a machine that cannot act on it.
        let invocation =
            crate::cli::parse(["--archive", "relative/path", "hook", SESSION_START]).unwrap();
        assert!(session_start(&invocation, Some(Path::new("/"))).is_err());
    }
}
