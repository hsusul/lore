//! `lorectl mem mine`: lessons mined from the archive's corrections, each
//! with its receipts. A dry run by design: it opens the archive read only and
//! writes nothing, to the archive or the vault.

use std::io::Write;

use lexopt::prelude::*;
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::storage;
use lore_memory::mine::{self, Candidate, Lesson, Signal};
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
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Short('h') | Long("help") => {
                print!("{usage_text}");
                return Ok(exit::OK);
            }
            Long("limit") => {
                let value = parser.value().map_err(|e| usage(e.to_string()))?;
                let value = value.to_string_lossy();
                limit = value.parse().ok().filter(|n| *n > 0).ok_or_else(|| {
                    usage(format!("--limit needs a positive number, got `{value}`"))
                })?;
            }
            Long("json") => json = true,
            other => return Err(usage(other.unexpected().to_string())),
        }
    }
    let conn = storage::open_read_only(&invocation.archive_dir()?.join(ARCHIVE_DB_FILENAME))?;
    let candidates = mine::corrections(&conn).map_err(storage::StorageError::from)?;
    let total = candidates.len();
    let mut lessons = mine::lessons(candidates);
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
            "{}  {} sessions, latest {}",
            lesson.id,
            lesson.sessions,
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
                "signal": match r.signal { Signal::Correction => "correction", Signal::Repeat => "repeat" },
            })
        })
        .collect();
    json!({
        "id": lesson.id, "text": lesson.text, "sessions": lesson.sessions,
        "receipts": receipts,
    })
}
