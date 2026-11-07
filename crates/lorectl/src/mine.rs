//! `lorectl mem mine`: lessons mined from the archive's corrections, each
//! with its receipts. A dry run by design: it opens the archive read only and
//! writes nothing, to the archive or the vault.
//!
//! `--propose ID...` files the chosen lessons into the vault's inbox through
//! the same path as every agent proposal, for a person to accept or reject.
//! A lesson is proposed as the agent of its newest receipt, under that
//! receipt's archive session id, marked verified: the miner read that session
//! from the archive, and it is where the person said it. It accepts nothing.
//!
//! `--new` lists only lessons with a correction no finished lessons run has
//! read, per the vault's considered ledger; `--json` marks each receipt
//! `new` either way. It records nothing in the ledger.
//!
//! `--replies` also lists, after them, the ordinary replies of the last
//! [`mine::REPLY_WINDOW_MS`] that no rule flagged, each on its own, as a
//! Find lessons run offers them.

use std::io::Write;
use std::path::{Path, PathBuf};

use lexopt::prelude::*;
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::storage;
use lore_memory::considered::{self, Marked};
use lore_memory::filing;
use lore_memory::inbox;
use lore_memory::mine::{self, Candidate, Lesson};
use lore_memory::review::ReviewError;
use lore_memory::vault::{self, Vault};
use serde_json::{json, Value};

use crate::cli::Invocation;
use crate::exit::{self, CliError};
use crate::receipt::date;

const DEFAULT_LIMIT: usize = 20;
/// Receipts printed per lesson; `--json` carries them all.
const SHOWN_RECEIPTS: usize = 3;

/// Run `lorectl mem mine`.
pub fn run(invocation: &Invocation, usage_text: &str) -> Result<u8, CliError> {
    let usage = |m: String| CliError::Usage(m);
    let mut parser = lexopt::Parser::from_args(&invocation.rest);
    let (mut limit, mut json) = (DEFAULT_LIMIT, invocation.json);
    let (mut propose, mut ids, mut vault_dir) = (false, Vec::new(), invocation.vault.clone());
    let mut limits = mine::Limits::default();
    let (mut all, mut new_only, mut replies) = (false, false, false);
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Short('h') | Long("help") => {
                print!("{usage_text}");
                return Ok(exit::OK);
            }
            Long("limit") => limit = number(&mut parser, "--limit", 1..=usize::MAX)?,
            Long("max-chars") => {
                limits.max_chars = number(&mut parser, "--max-chars", 1..=usize::MAX)?;
            }
            Long("min-overlap") => {
                let percent = number(&mut parser, "--min-overlap", 1..=100)?;
                limits.min_overlap_per_mille = percent * 10;
            }
            Long("json") => json = true,
            Long("all") => all = true,
            Long("new") => new_only = true,
            Long("replies") => replies = true,
            Long("propose") => propose = true,
            Long("vault") => {
                let value = parser.value().map_err(|e| usage(e.to_string()))?;
                vault_dir = Some(PathBuf::from(value));
            }
            Value(id) if propose => ids.push(id.to_string_lossy().into_owned()),
            other => return Err(usage(other.unexpected().to_string())),
        }
    }
    let conn = storage::open_read_only(&invocation.archive_dir()?.join(ARCHIVE_DB_FILENAME))?;
    let turns = mine::read_turns(&conn).map_err(storage::StorageError::from)?;
    let candidates = mine::corrections_in(&turns, &limits);
    let total = candidates.len();
    let mut lessons = mine::lessons_within(candidates, &limits);
    mine::find_conflicts(&turns, &mut lessons);
    if replies || propose {
        let since = lore_core::now_ms().saturating_sub(mine::REPLY_WINDOW_MS);
        lessons.extend(mine::reply_lessons(mine::replies_in(
            &turns, &limits, since,
        )));
    }
    if propose {
        return propose_lessons(&lessons, &ids, vault_dir.as_deref());
    }
    let before = lessons.len();
    if !all {
        lessons.retain(|l| l.score >= 0);
    }
    let left_out = before - lessons.len();
    // No vault, or none yet, means no ledger: every correction is new.
    let seen = vault::resolve_dir(vault_dir.as_deref())
        .ok()
        .and_then(|dir| Vault::open(&dir).ok())
        .map(|vault| considered::considered(&vault))
        .transpose()?
        .unwrap_or_default();
    let mut lessons = considered::mark(lessons, |id| !seen.contains(id), !new_only);
    lessons.truncate(limit);
    let text = if json {
        format!(
            "{}\n",
            Value::Array(lessons.iter().map(lesson_json).collect())
        )
    } else {
        lessons_text(&lessons, total, left_out)
    };
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
    Ok(exit::OK)
}

/// The next argument as a whole number in `range`, or a usage error.
fn number(
    parser: &mut lexopt::Parser,
    flag: &str,
    range: std::ops::RangeInclusive<usize>,
) -> Result<usize, CliError> {
    let value = parser.value().map_err(|e| CliError::Usage(e.to_string()))?;
    let value = value.to_string_lossy();
    let (low, high) = (range.start(), range.end());
    value
        .parse()
        .ok()
        .filter(|n| range.contains(n))
        .ok_or_else(|| {
            let wanted = if *high == usize::MAX {
                "a positive number".to_owned()
            } else {
                format!("a number from {low} to {high}")
            };
            CliError::Usage(format!("{flag} needs {wanted}, got `{value}`"))
        })
}

fn day(candidate: &Candidate) -> String {
    candidate
        .at_ms
        .and_then(date)
        .unwrap_or_else(|| "undated".into())
}

/// The lessons as text; `left_out` lessons scored as one-off orders were
/// not listed.
fn lessons_text(lessons: &[Marked], total: usize, left_out: usize) -> String {
    let plural = if lessons.len() == 1 { "" } else { "s" };
    let more = if left_out == 0 {
        String::new()
    } else {
        format!(", {left_out} more scored as one-off orders (--all shows them)")
    };
    let mut out = format!(
        "{} lesson{plural} from {total} corrections{more}. A dry run: nothing was written.\n",
        lessons.len()
    );
    for lesson in lessons.iter().map(Marked::lesson) {
        let newest = &lesson.receipts[0];
        out.push_str(&format!(
            "{}  {} conversation{}, latest {}",
            lesson.id,
            lesson.conversations,
            if lesson.conversations == 1 { "" } else { "s" },
            day(newest)
        ));
        if let Some(repo) = &newest.repo {
            out.push_str(&format!(", repo {repo}"));
        }
        out.push_str(&format!(
            "\n    \"{}\"\n    suggested: {}\n",
            lesson.text,
            mine::suggest_rule(&lesson.text)
        ));
        for (i, receipt) in lesson.receipts.iter().take(SHOWN_RECEIPTS).enumerate() {
            out.push_str(&format!(
                "    - {} {} {}: \"{}\"\n",
                receipt.agent,
                receipt.session,
                day(receipt),
                receipt.text
            ));
            if let Some(after) = receipt.preceding.as_ref().filter(|_| i == 0) {
                out.push_str(&format!("      after: \"{after}\"\n"));
            }
        }
        let more = lesson.receipts.len().saturating_sub(SHOWN_RECEIPTS);
        if more > 0 {
            out.push_str(&format!("    - and {more} more sessions\n"));
        }
        for later in &lesson.conflicts {
            out.push_str(&format!(
                "    - may conflict with {} {} {}: \"{}\"\n",
                later.agent,
                later.session,
                day(later),
                later.text
            ));
        }
    }
    out
}

fn lesson_json(marked: &Marked) -> Value {
    let lesson = marked.lesson();
    let receipts: Vec<Value> = (marked.receipts())
        .map(|(r, id, new)| {
            json!({
                "correction_id": id.as_str(), "new": new,
                "session": r.session, "agent": r.agent, "date": r.at_ms.and_then(date),
                "text": r.text, "after": r.preceding, "repo": r.repo,
                "signal": r.signal.as_str(),
            })
        })
        .collect();
    json!({
        "id": lesson.id, "text": lesson.text, "suggested_rule": mine::suggest_rule(&lesson.text),
        "conversations": lesson.conversations,
        "receipts": receipts,
        "may_conflict_with": lesson.conflicts.iter().map(|later| json!({
            "session": later.session, "agent": later.agent, "date": later.at_ms.and_then(date),
            "text": later.text,
        })).collect::<Vec<_>>(),
    })
}

/// File each lesson `ids` names. Every id is checked before anything is
/// written, and a lesson already in the inbox is not filed twice.
fn propose_lessons(
    lessons: &[Lesson],
    ids: &[String],
    vault: Option<&Path>,
) -> Result<u8, CliError> {
    if ids.is_empty() {
        return Err(CliError::Usage(
            "--propose needs at least one lesson id".into(),
        ));
    }
    let chosen = ids
        .iter()
        .map(|id| {
            lessons.iter().find(|l| l.id == *id).ok_or_else(|| {
                CliError::Usage(format!("no lesson `{id}`; `lorectl mem mine` lists them"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let vault = Vault::open(&vault::resolve_dir(vault)?)?;
    let (filed, _) = inbox::proposals(&vault).map_err(ReviewError::from)?;
    let mut out = std::io::stdout().lock();
    for lesson in chosen {
        if filing::is_filed(&filed, lesson) {
            let _ = writeln!(out, "already proposed lesson {}", lesson.id);
            continue;
        }
        let proposal = filing::proposal(lesson).map_err(|e| CliError::Usage(e.to_string()))?;
        let written = inbox::propose(&vault, proposal).map_err(ReviewError::from)?;
        let _ = writeln!(
            out,
            "proposed {} {}/{} for lesson {}",
            written.id.as_str(),
            inbox::INBOX_DIR,
            written.file_name(),
            lesson.id
        );
    }
    Ok(exit::OK)
}
