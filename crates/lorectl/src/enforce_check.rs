//! `lorectl mem enforce-check`: each active `enforce` rule, and how many of the
//! archive's recent tool calls it would have denied.
//!
//! A rule denies every call its pattern matches, so a person should see what
//! it would have caught before accepting it. This replays each rule against
//! the tool calls the archive recorded in the last [`DAYS`] days, read only,
//! and prints the count with a few examples. It writes nothing.
//!
//! The replay covers every archived call, whatever repository it ran in, so a
//! repository's rule may count calls the hook would never have seen there.
//! Tool inputs the archive keeps in a blob are not read.
//!
//! A `write` rule matches the lines a call adds, as in the hook: an `Edit`'s
//! lines of `new_string` not in `old_string`, an `apply_patch`'s `+` lines.
//! The archive does not keep the file an archived `Write` replaced, and the
//! file on disk today is not what it was then, so every line of a `Write`'s
//! content counts as added: the replay may count a rewrite the hook allows,
//! never miss one it denies.

use std::io::Write;
use std::path::{Path, PathBuf};

use lexopt::prelude::*;
use lore_core::paths::{self, ARCHIVE_DB_FILENAME};
use lore_core::storage;
use lore_memory::enforce::{Rule, Rules, ToolCall};
use lore_memory::vault::{self, Vault};
use serde_json::Value;

use crate::cli::Invocation;
use crate::exit::{self, CliError};

/// How far back the replay looks.
pub const DAYS: i64 = 30;
/// Examples printed per rule.
const EXAMPLES: usize = 3;
/// Characters of each example shown.
const EXAMPLE_CHARS: usize = 100;

/// One archived tool call: its day, as `YYYY-MM-DD`, and its parts.
struct Past {
    day: String,
    call: ToolCall,
}

/// Run `lorectl mem enforce-check`.
pub fn run(invocation: &Invocation, usage_text: &str) -> Result<u8, CliError> {
    let usage = |m: String| CliError::Usage(m);
    let mut parser = lexopt::Parser::from_args(&invocation.rest);
    let (mut vault_dir, mut cwd) = (invocation.vault.clone(), None);
    let mut archive_dir = invocation.archive.clone();
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Short('h') | Long("help") => {
                print!("{usage_text}");
                return Ok(exit::OK);
            }
            Long("vault") => {
                let value = parser.value().map_err(|e| usage(e.to_string()))?;
                vault_dir = Some(PathBuf::from(value));
            }
            Long("archive") => {
                let value = parser.value().map_err(|e| usage(e.to_string()))?;
                archive_dir = Some(PathBuf::from(value));
            }
            Long("cwd") => {
                let value = parser.value().map_err(|e| usage(e.to_string()))?;
                cwd = Some(PathBuf::from(value));
            }
            other => return Err(usage(other.unexpected().to_string())),
        }
    }
    let vault = Vault::open(&vault::resolve_dir(vault_dir.as_deref())?)?;
    let rules = Rules::load(&vault)?;
    for (rel, error) in &rules.broken {
        eprintln!("lorectl: {rel} blocks nothing: {error}");
    }
    for dropped in &rules.dropped {
        eprintln!(
            "lorectl: invalid rule in {} {} ({}): {}; the rule blocks nothing",
            dropped.id.as_str(),
            dropped.title,
            dropped.rel,
            dropped.error
        );
    }
    let chosen: Vec<&Rule> = match &cwd {
        Some(cwd) => rules.in_scope(cwd),
        None => rules.rules.iter().collect(),
    };
    let mut out = std::io::stdout().lock();
    if chosen.is_empty() {
        let _ = writeln!(out, "no active note has an enforce rule");
    } else {
        let past = history(archive_dir.as_deref());
        if past.is_none() {
            let _ = writeln!(out, "no archive to replay against; listing rules only\n");
        }
        for rule in &chosen {
            let _ = out.write_all(report(rule, past.as_deref()).as_bytes());
        }
    }
    Ok(if rules.broken.is_empty() && rules.dropped.is_empty() {
        exit::OK
    } else {
        exit::NOTE_REFUSED
    })
}

/// One rule's block: the note, the rule, and what the replay found.
fn report(rule: &Rule, past: Option<&[Past]>) -> String {
    let mut text = format!(
        "{}  {}  ({}, {})\n  enforce {}\n",
        rule.id.as_str(),
        rule.title,
        rule.scope,
        rule.rel,
        rule.enforcement.describe()
    );
    let Some(past) = past else {
        return text + "\n";
    };
    let hits: Vec<(&str, &str)> = past
        .iter()
        .filter_map(|p| {
            rule.enforcement
                .check(&p.call)
                .map(|found| (p.day.as_str(), found))
        })
        .collect();
    text.push_str(&format!(
        "  would have denied {} of {} archived tool calls in the last {DAYS} days\n",
        hits.len(),
        past.len()
    ));
    for (day, found) in hits.iter().take(EXAMPLES) {
        let shown: String = found
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(EXAMPLE_CHARS)
            .collect();
        text.push_str(&format!("    {day}  {shown}\n"));
    }
    text + "\n"
}

/// The archive's tool calls from the last [`DAYS`] days, oldest first, or
/// `None` when there is no readable archive.
fn history(archive: Option<&Path>) -> Option<Vec<Past>> {
    let dir = paths::archive_dir(archive).ok()?;
    let conn = storage::open_read_only(&dir.join(ARCHIVE_DB_FILENAME)).ok()?;
    let now_ms = time::OffsetDateTime::now_utc().unix_timestamp() * 1000;
    let since = now_ms - DAYS * 86_400_000;
    let mut stmt = conn
        .prepare(
            "SELECT tc.name, tc.input_json, m.ts FROM tool_call_full tc
             JOIN message_part p ON p.id = tc.call_part_id
             JOIN message m ON m.id = p.message_id
             WHERE m.ts >= ?1 AND tc.input_json IS NOT NULL
             ORDER BY m.ts",
        )
        .ok()?;
    let rows = stmt
        .query_map([since], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .ok()?;
    let mut past = Vec::new();
    for (name, input, ts) in rows.flatten() {
        let input = serde_json::from_str(&input).unwrap_or(Value::String(input));
        let call = ToolCall::from_tool(&name, &input);
        if !call.is_empty() {
            past.push(Past { day: day(ts), call });
        }
    }
    Some(past)
}

/// `ms` since the epoch as a UTC `YYYY-MM-DD`.
fn day(ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000))
        .map(|t| {
            let d = t.date();
            format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_format_in_utc() {
        assert_eq!(day(0), "1970-01-01");
        assert_eq!(day(86_400_000 + 5), "1970-01-02");
    }
}
