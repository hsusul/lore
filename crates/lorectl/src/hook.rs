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
//! `prompt` runs when a person sends a prompt. It recalls the notes that share
//! words with the prompt (see [`PROMPT_NOTE_CAP`] for which) and prints them
//! as the UserPromptSubmit `additionalContext` reply, so a lesson arrives
//! next to the request it is about rather than far back at session start.
//! A prompt with no content words, such as "yes", prints nothing. Nor does
//! a turn the host wrote rather than the person, such as a background task's
//! `<task-notification>` or a usage-limit note: recall reads only the
//! person's words, as the miner does (`lore_memory::mine::persons_words`),
//! so a typed request with a notification block in it recalls on the typed
//! words alone.
//!
//! `pre-edit` runs before a tool changes a file. It reads `tool_input.file_path`
//! (or `notebook_path`) from the hook JSON, or for Codex's `apply_patch` every
//! file the patch text in `tool_input` adds, updates, deletes, or
//! moves to, and prints the accepted notes anchored to those files, each with
//! its freshness label, as the PreToolUse `additionalContext` reply Claude
//! Code and Codex pass to the model. It never answers with a permission
//! decision, so it cannot allow or block the edit. No note on those files
//! prints nothing.
//!
//! `pre-tool` runs before any tool call the plugins route to it (shell
//! commands and file writes). It first checks the `enforce` rules of the
//! active notes in scope, global and the repository at `cwd`
//! (`lore_memory::enforce`). A `write` rule looks only at the lines a call
//! adds: for a `Write`, the lines of its content the file on disk (a path
//! relative to `cwd`, at most 2 MB, read only) does not already hold; for an
//! `Edit`, the lines of `new_string` not in `old_string`. The first rule the
//! call matches makes it print a
//! PreToolUse `deny` naming the note, and nothing else. When no rule matches,
//! it prints what `pre-edit` would for the same input, so one hook carries
//! both and an edit starts one process, not two. Reading the rules reads only
//! note files that have an `enforce:` key; an unreadable vault, or a rule
//! that does not parse, blocks nothing. `pre-edit` stays for configs
//! written before `pre-tool` existed.
//!
//! All four record the note ids they print for the hook JSON's `session_id`
//! (`lore_memory::shown`). `prompt` skips a note the session was already
//! shown. `session-start` and `pre-edit` print theirs regardless: the first
//! comes before anything else, and the second exists to repeat a note at the
//! moment its file is touched.
//!
//! Every part that reads the working tree at `cwd` first asks whether it
//! answers within [`WORKTREE_LIMIT`] (`crate::worktree`): macOS can hold an
//! open under Desktop or Documents until a person answers a privacy prompt.
//! One that does not answer is left out, as if `cwd` were not a repository:
//! `pre-tool` then checks only global rules, and a `Write` whose file on disk
//! cannot be read in time counts every line it writes as added.
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
use lore_memory::enforce::{self, Rules, ToolCall};
use lore_memory::mine;
use lore_memory::note::{RelPath, Scope};
use lore_memory::recall;
use lore_memory::session_start::{self, line};
use lore_memory::shown::Shown;
use lore_memory::vault::{self, Vault};

use crate::cli::Invocation;
use crate::exit::{self, CliError};
use crate::worktree;

/// The event names this build understands.
///
/// An unknown event is not an error: agents add hook points over time, and a
/// Lore that refused an unfamiliar one would break sessions on upgrade.
const SESSION_START: &str = "session-start";
const PRE_EDIT: &str = "pre-edit";
const PRE_TOOL: &str = "pre-tool";
const PROMPT: &str = "prompt";

/// At most this many notes before an edit, in at most this many bytes.
const EDIT_NOTE_CAP: usize = 5;
const EDIT_CHAR_CAP: usize = 1_000;
/// At most this many notes for a prompt, in at most this many bytes.
///
/// The relevance cut is the match itself, not a score: a note is a candidate
/// only when it holds at least two of the prompt's content words, or, when
/// the prompt has a single content word, its title holds that word
/// (`recall::related`).
/// Content words are `index::related_words`: contractions such as "don't",
/// single letters, and stopwords never count, so two texts do not match on
/// the pieces of a contraction.
/// At least one of the shared words must be in the note's lesson (its title,
/// or its body above the Receipts list) and must not be one most notes hold
/// (`index::Index::search_active_related`).
/// BM25 scores are not comparable from one vault or prompt to the next, so a
/// score threshold would be arbitrary, while a two-word overlap rules out
/// notes that share one common word with a long prompt. The candidates are
/// shown best first by BM25, with changed and gone notes last.
const PROMPT_NOTE_CAP: usize = 3;
const PROMPT_CHAR_CAP: usize = 800;
/// Recall this many for a prompt, so notes already shown can be skipped.
const PROMPT_CANDIDATES: usize = 10;
/// Hook JSON past this many bytes is not read.
const STDIN_CAP: u64 = 1 << 20;
/// The history walk behind the at-risk line gives up past this. Half the 10 s
/// the Claude Code plugin allows the hook, so the notes still get their turn.
const AT_RISK_BUDGET: Duration = Duration::from_secs(5);
/// How long a hook waits on the working tree at `cwd` before going on
/// without it. Well inside the 10 s the plugins allow the hook.
const WORKTREE_LIMIT: Duration = Duration::from_secs(1);

/// Run a hook event. Always returns `Ok(exit::OK)`.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let event = invocation.operand.as_deref().unwrap_or_default();
    if event == SESSION_START {
        let input = hook_input();
        let cwd = input.cwd.or_else(|| std::env::current_dir().ok());
        let cwd = cwd.filter(|cwd| worktree::discover(cwd, WORKTREE_LIMIT).is_ok());
        // Errors are swallowed on purpose: see the module docs.
        let _ = session_start(invocation, cwd.as_deref());
        let mut stdout = std::io::stdout().lock();
        if let Some(block) = memory_notes(cwd.as_deref(), input.session_id.as_deref()) {
            let _ = stdout.write_all(block.as_bytes());
        }
        if let Some(id) = input.session_id.and_then(|id| archived(invocation, &id)) {
            let _ = writeln!(stdout, "Lore session: {id}");
        }
    } else if event == PRE_EDIT {
        if let Some(text) = anchored_notes(&hook_input()) {
            reply("PreToolUse", &text);
        }
    } else if event == PRE_TOOL {
        let json = stdin_json().unwrap_or_default();
        let input = read_input(&json);
        if let Some(reason) = denial(&json, &input) {
            let reply = serde_json::json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason } });
            let _ = writeln!(std::io::stdout().lock(), "{reply}");
        } else if let Some(text) = anchored_notes(&input) {
            reply("PreToolUse", &text);
        }
    } else if event == PROMPT {
        if let Some(text) = prompt_notes(&hook_input()) {
            reply("UserPromptSubmit", &text);
        }
    }
    Ok(exit::OK)
}

/// Print `text` as the `additionalContext` of a hook reply for `event`.
/// Plain stdout before a tool call reaches no model; this field does.
fn reply(event: &str, text: &str) {
    let reply = serde_json::json!({ "hookSpecificOutput": {
        "hookEventName": event, "additionalContext": text } });
    let _ = writeln!(std::io::stdout().lock(), "{reply}");
}

/// What an agent's hook JSON on stdin says, as far as this hook uses it.
#[derive(Default)]
struct HookInput {
    /// The absolute `cwd`.
    cwd: Option<PathBuf>,
    session_id: Option<String>,
    /// The files a tool is about to change, as the agent wrote them.
    file_paths: Vec<PathBuf>,
    /// What the person just sent.
    prompt: Option<String>,
}

/// The reason the first `enforce` rule in scope that matches the tool call in
/// `json` gives, or `None` when none does or the vault cannot be read.
fn denial(json: &serde_json::Value, input: &HookInput) -> Option<String> {
    let name = json.get("tool_name").and_then(serde_json::Value::as_str)?;
    let tool_input = json.get("tool_input").cloned().unwrap_or_default();
    if ToolCall::from_tool(name, &tool_input).is_empty() {
        return None;
    }
    let vault = Vault::open(&vault::resolve_dir(None).ok()?).ok()?;
    let rules = Rules::load(&vault).ok()?;
    if rules.rules.is_empty() {
        return None;
    }
    let cwd = input.cwd.clone().or_else(|| std::env::current_dir().ok())?;
    // A `Write` matches only the lines the file does not already hold, so
    // rewriting a file never trips on text that was there before.
    let existing = (name == "Write")
        .then(|| {
            tool_input
                .get("file_path")
                .and_then(serde_json::Value::as_str)
        })
        .flatten()
        .filter(|path| !path.is_empty())
        .and_then(|path| {
            let file = cwd.join(path);
            worktree::within(&cwd, WORKTREE_LIMIT, move || enforce::read_existing(&file))
                .ok()
                .flatten()
        });
    let call = ToolCall::from_tool_over(name, &tool_input, existing.as_deref());
    let repo_rules = (rules.rules.iter()).any(|rule| !matches!(rule.scope, Scope::Global));
    let in_scope = if repo_rules && worktree::discover(&cwd, WORKTREE_LIMIT).is_err() {
        let global = rules.rules.iter();
        global
            .filter(|rule| matches!(rule.scope, Scope::Global))
            .collect()
    } else {
        rules.in_scope(&cwd)
    };
    let (rule, found) = enforce::first_denial(&in_scope, &call)?;
    Some(enforce::deny_reason(rule, found))
}

/// The hook JSON on stdin. Anything unreadable reads as empty.
fn hook_input() -> HookInput {
    read_input(&stdin_json().unwrap_or_default())
}

/// What `json`, an agent's hook JSON, says.
fn read_input(json: &serde_json::Value) -> HookInput {
    let text = |key: &str| json.get(key).and_then(serde_json::Value::as_str);
    HookInput {
        cwd: text("cwd")
            .map(PathBuf::from)
            .filter(|cwd| cwd.is_absolute()),
        session_id: text("session_id")
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
        file_paths: if text("tool_name") == Some("apply_patch") {
            let input = json.get("tool_input");
            let patch = input
                .and_then(|ti| {
                    ["command", "input", "patch"]
                        .iter()
                        .find_map(|key| ti.get(key).and_then(serde_json::Value::as_str))
                        .or_else(|| ti.as_str())
                })
                .unwrap_or_default();
            enforce::patch_paths(patch)
                .into_iter()
                .map(PathBuf::from)
                .collect()
        } else {
            ["/tool_input/file_path", "/tool_input/notebook_path"]
                .iter()
                .find_map(|at| json.pointer(at).and_then(serde_json::Value::as_str))
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .into_iter()
                .collect()
        },
        prompt: text("prompt").map(str::to_owned),
    }
}

/// The first JSON value on stdin. A terminal is never read, so a person
/// running the hook is not left waiting. Reading stops at the end of that
/// value, not at end of file: a parent that keeps the pipe open, or a child
/// that inherited its write end, would otherwise hold every hook until it
/// exits.
fn stdin_json() -> Option<serde_json::Value> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let reader = stdin.lock().take(STDIN_CAP);
    serde_json::Deserializer::from_reader(reader)
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()
}

/// The archive id of the session `id` names, read only. `None` when there is
/// no archive or it holds no such session.
fn archived(invocation: &Invocation, id: &str) -> Option<String> {
    let archive_dir = invocation.archive_dir().ok()?;
    let conn = storage::open_read_only(&archive_dir.join(ARCHIVE_DB_FILENAME)).ok()?;
    Some(query::find_session(&conn, id).ok()??.archive_id)
}

/// The notes section for `cwd`, or `None` when there is nothing to add. The
/// notes it holds are recorded as shown to `session_id`.
fn memory_notes(cwd: Option<&Path>, session_id: Option<&str>) -> Option<String> {
    let vault = Vault::open(&vault::resolve_dir(None).ok()?).ok()?;
    let found = session_start::session_start(&vault, cwd?).ok()??;
    let shown = found.notes.iter().take(found.shown);
    Shown::open(&vault, session_id).record(shown.map(|r| r.note.id.as_str()));
    Some(found.text)
}

/// The notes that match what the person just sent and the session has not
/// been shown, or `None` when there are none. See [`PROMPT_NOTE_CAP`].
fn prompt_notes(input: &HookInput) -> Option<String> {
    let prompt = mine::persons_words(input.prompt.as_deref()?)?;
    let cwd = input.cwd.clone().or_else(|| std::env::current_dir().ok())?;
    let vault = Vault::open(&vault::resolve_dir(None).ok()?).ok()?;
    worktree::discover(&cwd, WORKTREE_LIMIT).ok()?;
    let found = recall::related(&vault, &prompt, &cwd, PROMPT_CANDIDATES).ok()?;
    if found.notes.is_empty() {
        return None;
    }
    let mut shown = Shown::open(&vault, input.session_id.as_deref());
    let fresh = found
        .notes
        .iter()
        .filter(|r| !shown.contains(r.note.id.as_str()));
    let mut out = String::from("Lore notes for this request:\n");
    let mut ids = Vec::new();
    for recalled in fresh.take(PROMPT_NOTE_CAP) {
        let line = line(recalled);
        if out.len() + line.len() + 1 > PROMPT_CHAR_CAP {
            break;
        }
        out.push_str(&line);
        out.push('\n');
        ids.push(recalled.note.id.as_str());
    }
    if ids.is_empty() {
        return None;
    }
    shown.record(ids);
    Some(out)
}

/// The notes anchored to the files the hook JSON says a tool will change, a
/// heading per file, or `None` when there are none. A relative path is
/// relative to `cwd`. A note anchored to several of the files is listed once.
fn anchored_notes(input: &HookInput) -> Option<String> {
    let cwd = input.cwd.clone().or_else(|| std::env::current_dir().ok())?;
    if input.file_paths.is_empty() {
        return None;
    }
    let context = worktree::discover(&cwd, WORKTREE_LIMIT).ok()?;
    let root = context.root()?.to_path_buf();
    let vault = Vault::open(&vault::resolve_dir(None).ok()?).ok()?;
    // Look no further once the files read so far fill the cap.
    let (mut notes, mut count) = (Vec::new(), 0);
    for file in &input.file_paths {
        if count >= EDIT_NOTE_CAP {
            break;
        }
        let Some(rel) = repo_relative(&root, &cwd.join(file)) else {
            continue;
        };
        let Ok(found) = recall::anchored_to(&vault, &cwd, &rel, EDIT_NOTE_CAP) else {
            continue;
        };
        count += found.len();
        notes.push((rel, found));
    }
    let mut out = String::new();
    let mut ids: Vec<&str> = Vec::new();
    'files: for (rel, found) in &notes {
        let mut block = format!("Lore notes anchored to {}:\n", rel.as_str());
        let heading = block.len();
        for recalled in found {
            let id = recalled.note.id.as_str();
            if ids.contains(&id) {
                continue;
            }
            let line = line(recalled);
            let full = ids.len() >= EDIT_NOTE_CAP
                || out.len() + block.len() + line.len() + 1 > EDIT_CHAR_CAP;
            if full {
                if block.len() > heading {
                    out.push_str(&block);
                }
                break 'files;
            }
            block.push_str(&line);
            block.push('\n');
            ids.push(id);
        }
        if block.len() > heading {
            out.push_str(&block);
        }
    }
    if ids.is_empty() {
        return None;
    }
    Shown::open(&vault, input.session_id.as_deref()).record(ids);
    Some(out)
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
