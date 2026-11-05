//! Correction points, from the corrections Lore mines out of the archive.
//!
//! The archive says which session a correction came from and what it said;
//! only the Claude Code session file holds the exact lines a replay needs, so
//! each correction is located again in its file. A correction that cannot be
//! located is counted under its reason, never dropped silently: the skip
//! counts are part of what a reader must see to trust the case set.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use lore_memory::mine::{self, Candidate};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

use crate::error::Result;
use crate::session;
use crate::types::Case;

/// How many characters of the mined text must appear in the session line.
/// The miner stores text redacted and cut short, so the head is what survives.
const MATCH_CHARS: usize = 60;

/// Why a mined correction did not become a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Skip {
    NotClaudeCode,
    NotInArchive,
    SessionFileGone,
    EmptyCorrection,
    CorrectionLineNotFound,
    NoPromptBeforeCorrection,
    PromptUndated,
    CorrectionUndated,
    Duplicate,
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotClaudeCode => "not claude-code",
            Self::NotInArchive => "session not in archive",
            Self::SessionFileGone => "session file gone",
            Self::EmptyCorrection => "empty correction text",
            Self::CorrectionLineNotFound => "correction line not found",
            Self::NoPromptBeforeCorrection => "no prompt turn before correction",
            Self::PromptUndated => "prompt has no timestamp",
            Self::CorrectionUndated => "correction has no timestamp",
            Self::Duplicate => "duplicate case",
        })
    }
}

/// The cases found, and how many corrections each reason turned away.
#[derive(Debug, Default)]
pub struct Extraction {
    pub cases: Vec<Case>,
    pub skipped: BTreeMap<Skip, usize>,
}

impl Extraction {
    /// `cases N; skipped reason: n, ...`, the line the command prints.
    #[must_use]
    pub fn summary(&self) -> String {
        let skipped: Vec<String> = self
            .skipped
            .iter()
            .map(|(reason, n)| format!("{reason}: {n}"))
            .collect();
        format!(
            "cases {}; skipped {}",
            self.cases.len(),
            if skipped.is_empty() {
                "none".to_owned()
            } else {
                skipped.join(", ")
            }
        )
    }
}

/// Every mined correction as a case, with session files looked up under
/// `projects` (Claude Code's `projects` folder). `conn` is only read.
pub fn extract(conn: &Connection, projects: &Path) -> Result<Extraction> {
    let limits = mine::Limits::default();
    let turns = mine::read_turns(conn)?;
    let lessons = mine::lessons_within(mine::corrections_in(&turns, &limits), &limits);
    let mut out = Extraction::default();
    let mut seen = BTreeSet::new();
    for lesson in &lessons {
        for receipt in &lesson.receipts {
            match case_for(conn, projects, &lesson.id, receipt)? {
                Ok(case) if !seen.insert(case.id.clone()) => {
                    *out.skipped.entry(Skip::Duplicate).or_default() += 1;
                }
                Ok(case) => out.cases.push(case),
                Err(skip) => *out.skipped.entry(skip).or_default() += 1,
            }
        }
    }
    Ok(out)
}

/// Claude Code's session root: `$CLAUDE_CONFIG_DIR/projects`, or
/// `~/.claude/projects`.
#[must_use]
pub fn claude_projects_dir() -> Option<PathBuf> {
    claude_config_dir().map(|dir| dir.join("projects"))
}

/// Claude Code's config folder: `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
#[must_use]
pub fn claude_config_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
}

fn case_for(
    conn: &Connection,
    projects: &Path,
    lesson: &str,
    receipt: &Candidate,
) -> Result<std::result::Result<Case, Skip>> {
    if receipt.agent != "claude-code" {
        return Ok(Err(Skip::NotClaudeCode));
    }
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT native_session_id, primary_model FROM agent_session WHERE id = ?1",
            [&receipt.session],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((Some(native), model)) = row else {
        return Ok(Err(Skip::NotInArchive));
    };
    let Some(file) = session_file(projects, &native) else {
        return Ok(Err(Skip::SessionFileGone));
    };
    let Ok(entries) = session::read_lines(&file) else {
        return Ok(Err(Skip::SessionFileGone));
    };
    Ok(locate(&entries, receipt).map(|found| Case {
        id: format!("{}:{}", receipt.session, found.correction_line),
        lesson: lesson.to_owned(),
        session: receipt.session.clone(),
        native,
        model,
        file,
        prompt_line: found.prompt_line,
        correction_line: found.correction_line,
        correction_ms: found.correction_ms,
        cwd: session::cwd(found.prompt).map(PathBuf::from),
        branch: session::branch(found.prompt).map(str::to_owned),
        prompt_ms: found.prompt_ms,
        prompt: session::content_text(found.prompt),
        actions: session::assistant_actions(
            entries
                .get(found.prompt_line + 1..found.correction_line)
                .unwrap_or_default(),
        ),
        correction: session::content_text(found.correction),
        repo: receipt.repo.clone(),
        signal: receipt.signal.into(),
    }))
}

/// `<projects>/<any project>/<native>.jsonl`, the first in name order.
fn session_file(projects: &Path, native: &str) -> Option<PathBuf> {
    let name = format!("{native}.jsonl");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|dir| dir.join(&name))
        .find(|path| path.is_file())
}

struct Located<'a> {
    prompt_line: usize,
    correction_line: usize,
    prompt: &'a Value,
    correction: &'a Value,
    prompt_ms: i64,
    correction_ms: i64,
}

/// The correction's line and the human turn before it. When the text matches
/// several human lines (a person repeating themselves), the one closest in
/// time to the mined turn wins, then the earliest.
fn locate<'a>(entries: &'a [Value], receipt: &Candidate) -> std::result::Result<Located<'a>, Skip> {
    let needle: String = squash(&receipt.text).chars().take(MATCH_CHARS).collect();
    if needle.is_empty() {
        return Err(Skip::EmptyCorrection);
    }
    let hits = entries.iter().enumerate().filter(|(_, e)| {
        session::is_human(e) && squash(&session::content_text(e)).contains(&needle)
    });
    let distance = |e: &Value| match (receipt.at_ms, session::timestamp_ms(e)) {
        (Some(want), Some(at)) => want.abs_diff(at),
        _ => u64::MAX,
    };
    let (correction_line, correction) = hits
        .min_by_key(|(i, e)| (distance(e), *i))
        .ok_or(Skip::CorrectionLineNotFound)?;
    let (prompt_line, prompt) = entries
        .get(..correction_line)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .rev()
        .find(|(_, e)| session::is_human(e))
        .ok_or(Skip::NoPromptBeforeCorrection)?;
    Ok(Located {
        prompt_line,
        correction_line,
        prompt,
        correction,
        prompt_ms: session::timestamp_ms(prompt).ok_or(Skip::PromptUndated)?,
        correction_ms: session::timestamp_ms(correction).ok_or(Skip::CorrectionUndated)?,
    })
}

/// Runs of whitespace as one space, trimmed: the miner keeps a turn on one
/// line, and the session file does not.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lore_memory::mine::Signal;
    use serde_json::json;

    fn receipt(text: &str, at_ms: Option<i64>) -> Candidate {
        Candidate {
            session: "s_1".into(),
            agent: "claude-code".into(),
            repo: None,
            repo_path: None,
            at_ms,
            text: text.into(),
            preceding: None,
            following: None,
            signal: Signal::Correction,
        }
    }

    fn human(text: &str, ts: &str) -> Value {
        json!({"type": "user", "timestamp": ts, "message": {"content": text}})
    }

    #[test]
    fn a_repeated_correction_matches_the_line_nearest_in_time() {
        let entries = vec![
            human("do the thing", "2026-01-01T00:00:00Z"),
            human("no, not\nlike that", "2026-01-01T00:01:00Z"),
            human("again", "2026-01-01T00:02:00Z"),
            human("no, not like that", "2026-01-01T00:03:00Z"),
        ];
        let late = 1_767_225_780_000; // 00:03:00
        let found = locate(&entries, &receipt("no, not like that", Some(late)))
            .ok()
            .unwrap();
        assert_eq!((found.prompt_line, found.correction_line), (2, 3));
        let early = locate(&entries, &receipt("no, not like that", None))
            .ok()
            .unwrap();
        assert_eq!((early.prompt_line, early.correction_line), (0, 1));
    }

    #[test]
    fn misses_carry_their_reason() {
        let entries = vec![human("only turn", "2026-01-01T00:00:00Z")];
        assert!(matches!(
            locate(&entries, &receipt("absent", None)),
            Err(Skip::CorrectionLineNotFound)
        ));
        assert!(matches!(
            locate(&entries, &receipt("only turn", None)),
            Err(Skip::NoPromptBeforeCorrection)
        ));
        assert!(matches!(
            locate(&entries, &receipt("   ", None)),
            Err(Skip::EmptyCorrection)
        ));
    }
}
