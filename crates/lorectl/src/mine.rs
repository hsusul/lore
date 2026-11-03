//! `lorectl mem mine`: lessons mined from the archive's corrections, each
//! with its receipts. A dry run by design: it opens the archive read only and
//! writes nothing, to the archive or the vault.
//!
//! `--propose ID...` files the chosen lessons into the vault's inbox through
//! the same path as every agent proposal, for a person to accept or reject.
//! A lesson is proposed as the agent of its newest receipt, under that
//! receipt's archive session id, marked verified: the miner read that session
//! from the archive, and it is where the person said it. It accepts nothing.

use std::io::Write;
use std::path::{Path, PathBuf};

use lexopt::prelude::*;
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::storage;
use lore_memory::inbox::{self, Proposal};
use lore_memory::mine::{self, Candidate, Lesson, Signal};
use lore_memory::note::{AgentKind, NoteId, NoteType, Scope, SessionId};
use lore_memory::repo::RepoContext;
use lore_memory::review::ReviewError;
use lore_memory::vault::{self, Vault};
use serde_json::{json, Value};
use time::OffsetDateTime;

use crate::cli::Invocation;
use crate::exit::{self, CliError};
use crate::receipt::date;

const DEFAULT_LIMIT: usize = 20;
/// Receipts printed per lesson; `--json` carries them all.
const SHOWN_RECEIPTS: usize = 3;
/// Receipts written into a proposed lesson's body.
const PROPOSED_RECEIPTS: usize = 10;
/// The longest title a proposal gets, as over MCP.
const TITLE_CHARS: usize = 120;

/// Run `lorectl mem mine`.
pub fn run(invocation: &Invocation, usage_text: &str) -> Result<u8, CliError> {
    let usage = |m: String| CliError::Usage(m);
    let mut parser = lexopt::Parser::from_args(&invocation.rest);
    let (mut limit, mut json) = (DEFAULT_LIMIT, invocation.json);
    let (mut propose, mut ids, mut vault_dir) = (false, Vec::new(), None::<PathBuf>);
    let mut limits = mine::Limits::default();
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
    let candidates =
        mine::corrections_within(&conn, &limits).map_err(storage::StorageError::from)?;
    let total = candidates.len();
    let mut lessons = mine::lessons_within(candidates, &limits);
    if propose {
        return propose_lessons(&lessons, &ids, vault_dir.as_deref());
    }
    lessons.truncate(limit);
    let text = if json {
        format!(
            "{}\n",
            Value::Array(lessons.iter().map(lesson_json).collect())
        )
    } else {
        lessons_text(&lessons, total)
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

fn lessons_text(lessons: &[Lesson], total: usize) -> String {
    let plural = if lessons.len() == 1 { "" } else { "s" };
    let mut out = format!(
        "{} lesson{plural} from {total} corrections. A dry run: nothing was written.\n",
        lessons.len()
    );
    for lesson in lessons {
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
        out.push_str(&format!("\n    \"{}\"\n", lesson.text));
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
    }
    out
}

fn lesson_json(lesson: &Lesson) -> Value {
    let receipts: Vec<Value> = (lesson.receipts.iter())
        .map(|r| {
            json!({
                "session": r.session, "agent": r.agent, "date": r.at_ms.and_then(date),
                "text": r.text, "after": r.preceding, "repo": r.repo,
                "signal": match r.signal { Signal::Correction => "correction", Signal::Repeat => "repeat", Signal::Rule => "rule" },
            })
        })
        .collect();
    json!({
        "id": lesson.id, "text": lesson.text, "conversations": lesson.conversations,
        "receipts": receipts,
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
        let marker = format!("(lesson {})", lesson.id);
        if filed.iter().any(|p| p.body.contains(&marker)) {
            let _ = writeln!(out, "already proposed lesson {}", lesson.id);
            continue;
        }
        let written =
            inbox::propose(&vault, proposal(lesson, &marker)?).map_err(ReviewError::from)?;
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

fn proposal(lesson: &Lesson, marker: &str) -> Result<Proposal, CliError> {
    let newest = &lesson.receipts[0];
    let bad = |e: lore_memory::note::InvalidValue| CliError::Usage(e.to_string());
    let mut body = format!(
        "{}\n\nMined by `lorectl mem mine` {marker} from corrections in {} conversation{}:\n",
        lesson.text,
        lesson.conversations,
        if lesson.conversations == 1 { "" } else { "s" }
    );
    for receipt in lesson.receipts.iter().take(PROPOSED_RECEIPTS) {
        body.push_str(&format!(
            "- {} {} {}: \"{}\"",
            day(receipt),
            receipt.agent,
            receipt.session,
            receipt.text
        ));
        if let Some(after) = &receipt.preceding {
            body.push_str(&format!(", after \"{after}\""));
        }
        body.push('\n');
    }
    let more = lesson.receipts.len().saturating_sub(PROPOSED_RECEIPTS);
    if more > 0 {
        body.push_str(&format!("- and {more} more sessions\n"));
    }
    Ok(Proposal {
        id: NoteId::generate(),
        agent: AgentKind::parse(&newest.agent).map_err(bad)?,
        session: SessionId::parse(&newest.session).map_err(bad)?,
        source_verified: true,
        note_type: NoteType::Convention,
        scope: scope(lesson),
        title: lesson.text.chars().take(TITLE_CHARS).collect(),
        body,
        anchors: Vec::new(),
        relation: None,
        proposed_at: OffsetDateTime::now_utc(),
    })
}

/// The repository every receipt came from, when they share one this machine
/// can still find; otherwise global.
fn scope(lesson: &Lesson) -> Scope {
    let mut paths = lesson.receipts.iter().map(|r| r.repo_path.as_deref());
    let first = paths.next().flatten();
    match first.filter(|first| paths.all(|p| p == Some(first))) {
        Some(path) => RepoContext::discover(Path::new(path))
            .key()
            .map_or(Scope::Global, |key| Scope::Repo(key.clone())),
        None => Scope::Global,
    }
}
