//! `lorectl init`: the first run. Makes the vault, scans recent sessions into
//! the archive, lists the proposals agents left in the inbox, and shows the
//! lessons in the person's corrections, each with a receipt. Safe to run
//! again: the vault is reused and the scan ingests only what changed. A
//! failure after the vault step is printed on that step's line and the next
//! step still runs.
//!
//! On a terminal, or with `--review`, each pending proposal is offered first:
//! accept, reject with a reason, skip, or quit. Then each lesson: accept the
//! suggested rule, edit it, skip, or quit. A kept lesson is filed and
//! accepted as a note in one step, so `mem eval` counts it as decided.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use lexopt::prelude::*;
use lore_core::paths::{self, ARCHIVE_DB_FILENAME};
use lore_core::storage::{self, StorageError};
use lore_memory::inbox::Proposal;
use lore_memory::mine::{self, Lesson};
use lore_memory::review;
use lore_memory::vault::{self, Init, Vault};

use crate::cli::Invocation;
use crate::exit::{self, CliError};
use crate::receipt::{date, Archive};

const DEFAULT_SINCE: &str = "30d";
const DEFAULT_LIMIT: usize = 5;

const NEXT: &str = "\
Next
  Better lessons: in Claude Code, run /lore:lore-lessons, then lorectl init --review
  Keep one:       lorectl mem mine --propose <id>, then lorectl mem accept
  Connect agents: lorectl connect
";

struct Args {
    since: String,
    limit: usize,
    vault: Option<PathBuf>,
    archive: Option<PathBuf>,
    /// `--review` or `--no-review`, the last one given.
    review: Option<bool>,
}

/// The flags after `init`. `None` when help was asked for.
fn parse(invocation: &Invocation) -> Result<Option<Args>, CliError> {
    let usage = |e: lexopt::Error| CliError::Usage(e.to_string());
    let mut parser = lexopt::Parser::from_args(&invocation.rest);
    let mut args = Args {
        since: (invocation.since.clone()).unwrap_or_else(|| DEFAULT_SINCE.into()),
        limit: DEFAULT_LIMIT,
        vault: None,
        archive: invocation.archive.clone(),
        review: None,
    };
    while let Some(arg) = parser.next().map_err(usage)? {
        match arg {
            Short('h') | Long("help") => return Ok(None),
            Long("since") => args.since = parser.value().map_err(usage)?.to_string_lossy().into(),
            Long("limit") => {
                let value = parser
                    .value()
                    .map_err(usage)?
                    .to_string_lossy()
                    .into_owned();
                args.limit = value.parse().ok().filter(|n| *n > 0).ok_or_else(|| {
                    CliError::Usage(format!("--limit needs a positive number, got `{value}`"))
                })?;
            }
            Long("vault") => args.vault = Some(parser.value().map_err(usage)?.into()),
            Long("archive") => args.archive = Some(parser.value().map_err(usage)?.into()),
            Long("review") => args.review = Some(true),
            Long("no-review") => args.review = Some(false),
            other => return Err(usage(other.unexpected())),
        }
    }
    Ok(Some(args))
}

/// Run `lorectl init`. Exits 0 unless the vault step fails.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let Some(args) = parse(invocation)? else {
        print!("{}", crate::help::usage());
        return Ok(exit::OK);
    };
    let (window, since) = crate::scan::window(&args.since)?;
    let archive_dir = paths::archive_dir(args.archive.as_deref())?;
    let vault_dir = vault::resolve_dir(args.vault.as_deref())?;
    let terminal = std::io::stdin().is_terminal();
    let review = args.review.unwrap_or(terminal);
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "Lore");

    let vault = match vault_step(&vault_dir) {
        Ok((vault, line)) => {
            let _ = writeln!(out, "Vault      {line}");
            vault
        }
        Err(error) => {
            let _ = writeln!(out, "Vault      failed: {error}");
            return Ok(exit::exit_code(&error));
        }
    };

    let started = Instant::now();
    let scanned = crate::scan::scan_into(&archive_dir, Some(since));
    let secs = started.elapsed().as_secs();
    let line = match scanned {
        Ok((_, _, summary)) => {
            let mut line = match summary.ingested {
                0 => "no new sessions".to_owned(),
                n => format!("scanned {n} new session{}", plural(n)),
            };
            line.push_str(&format!(" from the last {window} in {secs} s"));
            if summary.failed > 0 {
                line.push_str(&format!(", {} failed", summary.failed));
            }
            line
        }
        Err(error) => format!("failed: {error}"),
    };
    let _ = writeln!(out, "Archive    {line}");

    let proposals = match review::inbox(&vault) {
        Ok(pending) => pending.proposals,
        Err(error) => {
            let _ = writeln!(out, "Inbox      failed: {error}\n");
            Vec::new()
        }
    };
    let archive = Archive::open(Some(&archive_dir));
    if !proposals.is_empty() {
        let n = proposals.len();
        let _ = writeln!(
            out,
            "Inbox      {n} proposal{} waiting for review:\n",
            plural(n)
        );
    }
    let lessons = lessons(&archive_dir);
    let has_lessons = matches!(&lessons, Ok(lessons) if !lessons.is_empty());
    if !(review && (has_lessons || !proposals.is_empty())) {
        for proposal in &proposals {
            let _ = writeln!(out, "{}", proposal_text(proposal, &archive));
        }
        match lessons {
            Ok(lessons) => drop(write!(out, "{}{NEXT}", lessons_text(&lessons, args.limit))),
            Err(error) => drop(write!(out, "Lessons    failed: {error}\n\n{NEXT}")),
        }
        return Ok(exit::OK);
    }

    let mut input = std::io::stdin().lock();
    // Piped answers are not echoed, so print them as a terminal would.
    let echo = !terminal;
    let (mut n, go_on) = review_proposals(&mut out, &mut input, echo, &vault, &archive, &proposals);
    if go_on {
        match lessons {
            Ok(lessons) => {
                let _ = write!(out, "{}", header(&lessons, args.limit));
                let shown = &lessons[..lessons.len().min(args.limit)];
                n += review_lessons(&mut out, &mut input, echo, &vault, shown);
            }
            Err(error) => drop(writeln!(out, "Lessons    failed: {error}\n")),
        }
    }
    let _ = writeln!(
        out,
        "Kept {n} lesson{}. Your next Claude Code or Codex session will see them \
         once Lore is connected (lorectl connect).",
        plural(n)
    );
    Ok(exit::OK)
}

/// One pending proposal: its id and title, its source, and the receipt
/// `mem inbox` prints for it.
fn proposal_text(proposal: &Proposal, archive: &Archive) -> String {
    let checked = if proposal.source_verified {
        "verified"
    } else {
        "unverified"
    };
    let text = crate::review::receipt_text(proposal);
    let receipt = archive.receipt(proposal.session.as_str(), &text);
    format!(
        "  {}  {}\n      {} ({checked})\n  {}\n",
        proposal.id.as_str(),
        proposal.title,
        proposal.source(),
        receipt.line()
    )
}

const INBOX_PROMPT: &str = "      [a]ccept  [r]eject  [s]kip  [q]uit ";
const DEFAULT_REASON: &str = "rejected in lorectl init";

enum Verdict {
    Accept,
    Reject(String),
    Skip,
}

/// Offer each pending proposal in turn. Returns how many were accepted, and
/// false when the person quit, so the lessons are not offered.
fn review_proposals(
    out: &mut impl Write,
    input: &mut impl BufRead,
    echo: bool,
    vault: &Vault,
    archive: &Archive,
    proposals: &[Proposal],
) -> (usize, bool) {
    let mut accepted = 0;
    for proposal in proposals {
        let _ = write!(out, "{}", proposal_text(proposal, archive));
        let Some(verdict) = decide(|prompt| answer(out, input, echo, prompt)) else {
            let _ = writeln!(out);
            return (accepted, false);
        };
        let id = proposal.id.as_str();
        let done = match verdict {
            Verdict::Accept => review::accept(vault, id, None).map(|done| {
                accepted += 1;
                let verb = if done.updated { "updated" } else { "accepted" };
                format!("      {verb} {}\n", done.rel)
            }),
            Verdict::Reject(reason) => {
                review::reject(vault, id, &reason).map(|_| "      rejected\n".to_owned())
            }
            Verdict::Skip => Ok(String::new()),
        };
        match done {
            Ok(line) => drop(writeln!(out, "{line}")),
            Err(error) => drop(writeln!(out, "      failed: {error}\n")),
        }
    }
    (accepted, true)
}

/// What to do with a proposal, or `None` to stop. A stray answer is asked
/// once more, then counts as a skip.
fn decide(mut ask: impl FnMut(&str) -> Option<String>) -> Option<Verdict> {
    for _ in 0..2 {
        match ask(INBOX_PROMPT)?.as_str() {
            "a" => return Some(Verdict::Accept),
            "r" => {
                let reason = ask("      reason: ")?;
                let reason = if reason.is_empty() {
                    DEFAULT_REASON.to_owned()
                } else {
                    reason
                };
                return Some(Verdict::Reject(reason));
            }
            "s" => return Some(Verdict::Skip),
            "q" => return None,
            _ => {}
        }
    }
    Some(Verdict::Skip)
}

/// Create the vault, or report the one already there and its note count.
fn vault_step(dir: &Path) -> Result<(Vault, String), CliError> {
    let (vault, init) = Vault::init(dir)?;
    if init == Init::Created {
        return Ok((vault, format!("created {}", dir.display())));
    }
    let notes = vault.index().and_then(|index| Ok(index.len()?))?;
    let line = format!("{}, {notes} note{}", dir.display(), plural(notes));
    Ok((vault, line))
}

const PROMPT: &str = "      [a]ccept  [e]dit  [s]kip  [q]uit ";

/// Offer each lesson in turn and keep the ones accepted. A lesson filed
/// before is not offered; its line says whether it was kept, is waiting in
/// the inbox, or was rejected. Returns how many were kept.
fn review_lessons(
    out: &mut impl Write,
    input: &mut impl BufRead,
    echo: bool,
    vault: &Vault,
    lessons: &[Lesson],
) -> usize {
    // An unreadable inbox reads as empty; filing then reports the error.
    let filed = lore_memory::inbox::proposals(vault).map_or_else(|_| Vec::new(), |(p, _)| p);
    let decisions = review::decisions(vault).unwrap_or_default();
    let mut kept = 0;
    for lesson in lessons {
        let _ = write!(out, "{}", lesson_text(lesson));
        if let Some(state) = filed_state(&filed, &decisions, lesson) {
            let _ = writeln!(out, "      ({state})\n");
            continue;
        }
        let suggested = mine::suggest_rule(&lesson.text);
        let Some(title) = choose(|prompt| answer(out, input, echo, prompt), suggested) else {
            let _ = writeln!(out);
            break;
        };
        if let Some(title) = title {
            match crate::mine::keep(vault, lesson, &title) {
                Ok(rel) => {
                    kept += 1;
                    let _ = writeln!(out, "      kept {rel}");
                }
                Err(error) => drop(writeln!(out, "      failed: {error}")),
            }
        }
        let _ = writeln!(out);
    }
    kept
}

/// What became of `lesson` if it was filed before: "already kept" when a
/// proposal from it was accepted, else "in the inbox" while one is pending,
/// else "rejected". `None` when it was never filed.
fn filed_state(
    filed: &[Proposal],
    decisions: &std::collections::HashMap<lore_memory::note::NoteId, review::Decision>,
    lesson: &Lesson,
) -> Option<&'static str> {
    let rank = |p: &Proposal| match decisions.get(&p.id) {
        Some(review::Decision::Accepted { .. }) => 0,
        None => 1,
        Some(review::Decision::Rejected { .. }) => 2,
    };
    let best = filed
        .iter()
        .filter(|p| crate::mine::came_from(p, lesson))
        .map(rank)
        .min()?;
    Some(["already kept", "in the inbox", "rejected"][best])
}

/// The title to keep a lesson under, `Some(None)` to skip it, or `None` to
/// stop. A stray answer is asked once more, then counts as a skip.
fn choose(
    mut ask: impl FnMut(&str) -> Option<String>,
    suggested: String,
) -> Option<Option<String>> {
    for _ in 0..2 {
        match ask(PROMPT)?.as_str() {
            "a" => return Some(Some(suggested)),
            "e" => {
                let text = ask("      rule: ")?;
                return Some(Some(if text.is_empty() { suggested } else { text }));
            }
            "s" => return Some(None),
            "q" => return None,
            _ => {}
        }
    }
    Some(None)
}

/// Print `prompt` and read one trimmed line. `None` at the end of input.
fn answer(
    out: &mut impl Write,
    input: &mut impl BufRead,
    echo: bool,
    prompt: &str,
) -> Option<String> {
    let _ = write!(out, "{prompt}");
    let _ = out.flush();
    let mut line = String::new();
    if input.read_line(&mut line).unwrap_or(0) == 0 {
        let _ = writeln!(out);
        return None;
    }
    let line = line.trim().to_owned();
    if echo {
        let _ = writeln!(out, "{line}");
    }
    Some(line)
}

/// Every mined lesson with a rule score of 0 or more, in the miner's order.
fn lessons(archive_dir: &Path) -> Result<Vec<Lesson>, CliError> {
    let conn = storage::open_read_only(&archive_dir.join(ARCHIVE_DB_FILENAME))?;
    let limits = mine::Limits::default();
    let candidates = mine::corrections_within(&conn, &limits).map_err(StorageError::from)?;
    let mut lessons = mine::lessons_within(candidates, &limits);
    lessons.retain(|lesson| lesson.score >= 0);
    Ok(lessons)
}

fn lessons_text(lessons: &[Lesson], limit: usize) -> String {
    let mut out = header(lessons, limit);
    for lesson in lessons.iter().take(limit) {
        out.push_str(&lesson_text(lesson));
        out.push('\n');
    }
    out
}

/// The `Lessons` line, and the blank line after it.
fn header(lessons: &[Lesson], limit: usize) -> String {
    let count = lessons.len();
    if count == 0 {
        return "Lessons    none found in your sessions yet.\n\n".into();
    }
    let shown = if count > limit {
        format!(". The top {limit}:")
    } else {
        ":".into()
    };
    format!(
        "Lessons    {count} candidate{} from your corrections{shown}\n\n",
        plural(count)
    )
}

/// One lesson: its id, the person's words, the suggested rule, and the
/// newest receipt.
fn lesson_text(lesson: &Lesson) -> String {
    let newest = &lesson.receipts[0];
    let day = newest
        .at_ms
        .and_then(date)
        .unwrap_or_else(|| "undated".into());
    format!(
        "  {}  {} conversation{}, latest {day}\n      \"{}\"\n      suggested: {}\n      - {} {} {day}: \"{}\"\n",
        lesson.id,
        lesson.conversations,
        plural(lesson.conversations),
        lesson.text,
        mine::suggest_rule(&lesson.text),
        newest.agent,
        newest.session,
        newest.text,
    )
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}
