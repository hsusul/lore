//! `lorectl init`: the first run. Makes the vault, scans recent sessions into
//! the archive, and shows the lessons in the person's corrections, each with a
//! receipt. Safe to run again: the vault is reused and the scan ingests only
//! what changed. A failure after the vault step is printed on that step's line
//! and the next step still runs.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use lexopt::prelude::*;
use lore_core::paths::{self, ARCHIVE_DB_FILENAME};
use lore_core::storage::{self, StorageError};
use lore_memory::mine::{self, Lesson};
use lore_memory::vault::{self, Init, Vault};

use crate::cli::Invocation;
use crate::exit::{self, CliError};
use crate::receipt::date;

const DEFAULT_SINCE: &str = "30d";
const DEFAULT_LIMIT: usize = 5;

const NEXT: &str = "\
Next
  Keep one:       lorectl mem mine --propose <id>, then lorectl mem accept
  Better wording: ask Claude Code or Codex to call mine_candidates and propose lessons
  Connect agents: lorectl connect
";

struct Args {
    since: String,
    limit: usize,
    vault: Option<PathBuf>,
    archive: Option<PathBuf>,
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
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "Lore");

    match vault_step(&vault_dir) {
        Ok(line) => drop(writeln!(out, "Vault      {line}")),
        Err(error) => {
            let _ = writeln!(out, "Vault      failed: {error}");
            return Ok(exit::exit_code(&error));
        }
    }

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

    let lessons = match lessons(&archive_dir) {
        Ok(lessons) => lessons_text(&lessons, args.limit),
        Err(error) => format!("Lessons    failed: {error}\n\n"),
    };
    let _ = write!(out, "{lessons}{NEXT}");
    Ok(exit::OK)
}

/// Create the vault, or report the one already there and its note count.
fn vault_step(dir: &Path) -> Result<String, CliError> {
    let (vault, init) = Vault::init(dir)?;
    if init == Init::Created {
        return Ok(format!("created {}", dir.display()));
    }
    let notes = vault.index().and_then(|index| Ok(index.len()?))?;
    Ok(format!("{}, {notes} note{}", dir.display(), plural(notes)))
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
    let count = lessons.len();
    if count == 0 {
        return "Lessons    none found in your sessions yet.\n\n".into();
    }
    let mut out = format!(
        "Lessons    {count} candidate{} from your corrections",
        plural(count)
    );
    out.push_str(&if count > limit {
        format!(". The top {limit}:\n\n")
    } else {
        ":\n\n".into()
    });
    for lesson in lessons.iter().take(limit) {
        let newest = &lesson.receipts[0];
        let day = newest
            .at_ms
            .and_then(date)
            .unwrap_or_else(|| "undated".into());
        out.push_str(&format!(
            "  {}  {} conversation{}, latest {day}\n      \"{}\"\n      - {} {} {day}: \"{}\"\n\n",
            lesson.id,
            lesson.conversations,
            plural(lesson.conversations),
            lesson.text,
            newest.agent,
            newest.session,
            newest.text,
        ));
    }
    out
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}
