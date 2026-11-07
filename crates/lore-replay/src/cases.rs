//! Correction points, from the corrections Lore mines out of the archive.
//!
//! The archive says which session a correction came from and what it said;
//! only the agent's own session file (a Claude Code session or a Codex
//! rollout) holds the exact lines a replay needs, so each correction is
//! located again in its file. A correction that cannot be
//! located is counted under its reason, never dropped silently: the skip
//! counts are part of what a reader must see to trust the case set.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use lore_memory::mine::{self, Candidate};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

use crate::codex;
use crate::error::Result;
use crate::session;
use crate::types::{Action, Agent, Case};

/// How many characters of the mined text must appear in the session line.
/// The miner stores text redacted and cut short, so the head is what survives.
const MATCH_CHARS: usize = 60;

/// Why a mined correction did not become a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Skip {
    UnsupportedAgent,
    NotInArchive,
    SessionFileGone,
    CodexSubagent,
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
            Self::UnsupportedAgent => "not claude-code or codex",
            Self::NotInArchive => "session not in archive",
            Self::SessionFileGone => "session file gone",
            Self::CodexSubagent => "sub-agent rollout, resumable only through its parent",
            Self::EmptyCorrection => "empty correction text",
            Self::CorrectionLineNotFound => "correction line not found",
            Self::NoPromptBeforeCorrection => "no prompt turn before correction",
            Self::PromptUndated => "prompt has no timestamp",
            Self::CorrectionUndated => "correction has no timestamp",
            Self::Duplicate => "duplicate case",
        })
    }
}

/// The cases found, and how many corrections each reason turned away, by
/// agent (the archive's `agent_id` for one replay does not run).
#[derive(Debug, Default)]
pub struct Extraction {
    pub cases: Vec<Case>,
    pub skipped: BTreeMap<(String, Skip), usize>,
}

impl Extraction {
    /// `cases N (claude-code a, codex b); skipped reason: n, ...`, the line
    /// the command prints. Skips are counted per agent.
    #[must_use]
    pub fn summary(&self) -> String {
        let skipped: Vec<String> = self
            .skipped
            .iter()
            .map(|((agent, reason), n)| format!("{agent} {reason}: {n}"))
            .collect();
        let per_agent = [Agent::ClaudeCode, Agent::Codex]
            .map(|agent| {
                let n = self.cases.iter().filter(|c| c.agent == agent).count();
                format!("{agent} {n}")
            })
            .join(", ");
        format!(
            "cases {} ({per_agent}); skipped {}",
            self.cases.len(),
            if skipped.is_empty() {
                "none".to_owned()
            } else {
                skipped.join(", ")
            }
        )
    }
}

/// Where the agents' session files are.
#[derive(Debug, Clone, Default)]
pub struct Roots {
    /// Claude Code's `projects` folder.
    pub claude_projects: Option<PathBuf>,
    /// Codex's home folder (`$CODEX_HOME`), whose `sessions` and
    /// `archived_sessions` hold the rollouts.
    pub codex_home: Option<PathBuf>,
}

/// Every mined correction as a case, with session files looked up under
/// `roots`. `conn` and the session files are only read.
pub fn extract(conn: &Connection, roots: &Roots) -> Result<Extraction> {
    let limits = mine::Limits::default();
    let turns = mine::read_turns(conn)?;
    let lessons = mine::lessons_within(mine::corrections_in(&turns, &limits), &limits);
    let mut out = Extraction::default();
    let mut seen = BTreeSet::new();
    let rollouts = roots
        .codex_home
        .as_deref()
        .map(codex::rollout_index)
        .unwrap_or_default();
    let files = Files {
        projects: roots.claude_projects.as_deref(),
        rollouts: &rollouts,
    };
    for lesson in &lessons {
        for receipt in &lesson.receipts {
            let mut skip = |skip| {
                *out.skipped
                    .entry((receipt.agent.clone(), skip))
                    .or_default() += 1;
            };
            match case_for(conn, &files, &lesson.id, receipt)? {
                Ok(case) if !seen.insert(case.id.clone()) => skip(Skip::Duplicate),
                Ok(case) => out.cases.push(case),
                Err(reason) => skip(reason),
            }
        }
    }
    Ok(out)
}

struct Files<'a> {
    projects: Option<&'a Path>,
    rollouts: &'a HashMap<String, PathBuf>,
}

/// How one agent's session file reads: which lines a person typed, and the
/// text, place, and actions around them.
struct Format {
    is_human: fn(&Value) -> bool,
    text: fn(&Value) -> String,
    actions: fn(&[Value]) -> Vec<Action>,
}

const CLAUDE: Format = Format {
    is_human: session::is_human,
    text: session::content_text,
    actions: session::assistant_actions,
};

const CODEX: Format = Format {
    is_human: codex::is_human,
    text: codex::content_text,
    actions: codex::assistant_actions,
};

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
    files: &Files<'_>,
    lesson: &str,
    receipt: &Candidate,
) -> Result<std::result::Result<Case, Skip>> {
    let Some(agent) = Agent::from_archive(&receipt.agent) else {
        return Ok(Err(Skip::UnsupportedAgent));
    };
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
    let (file, format) = match agent {
        Agent::ClaudeCode => (
            files.projects.and_then(|p| session_file(p, &native)),
            &CLAUDE,
        ),
        Agent::Codex => (files.rollouts.get(&native).cloned(), &CODEX),
    };
    let Some(file) = file else {
        return Ok(Err(Skip::SessionFileGone));
    };
    let Ok(entries) = session::read_lines(&file) else {
        return Ok(Err(Skip::SessionFileGone));
    };
    if agent == Agent::Codex && codex::is_subagent(&entries) {
        return Ok(Err(Skip::CodexSubagent));
    }
    let codex_branch = match agent {
        Agent::ClaudeCode => None,
        Agent::Codex => codex::branch(&entries),
    };
    Ok(locate(&entries, receipt, format).map(|found| Case {
        id: format!("{}:{}", receipt.session, found.correction_line),
        agent,
        lesson: lesson.to_owned(),
        session: receipt.session.clone(),
        native,
        model,
        file,
        prompt_line: found.prompt_line,
        correction_line: found.correction_line,
        correction_ms: found.correction_ms,
        cwd: match agent {
            Agent::ClaudeCode => session::cwd(found.prompt).map(PathBuf::from),
            Agent::Codex => codex::cwd_at(&entries, found.prompt_line).map(PathBuf::from),
        },
        branch: match agent {
            Agent::ClaudeCode => session::branch(found.prompt).map(str::to_owned),
            Agent::Codex => codex_branch.clone(),
        },
        prompt_ms: found.prompt_ms,
        prompt: (format.text)(found.prompt),
        actions: (format.actions)(
            entries
                .get(found.prompt_line + 1..found.correction_line)
                .unwrap_or_default(),
        ),
        correction: (format.text)(found.correction),
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
fn locate<'a>(
    entries: &'a [Value],
    receipt: &Candidate,
    format: &Format,
) -> std::result::Result<Located<'a>, Skip> {
    let needle: String = squash(&receipt.text).chars().take(MATCH_CHARS).collect();
    if needle.is_empty() {
        return Err(Skip::EmptyCorrection);
    }
    let hits = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| (format.is_human)(e) && squash(&(format.text)(e)).contains(&needle));
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
        .find(|(_, e)| (format.is_human)(e))
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
        let found = locate(&entries, &receipt("no, not like that", Some(late)), &CLAUDE)
            .ok()
            .unwrap();
        assert_eq!((found.prompt_line, found.correction_line), (2, 3));
        let early = locate(&entries, &receipt("no, not like that", None), &CLAUDE)
            .ok()
            .unwrap();
        assert_eq!((early.prompt_line, early.correction_line), (0, 1));
    }

    #[test]
    fn a_codex_correction_becomes_a_codex_case() {
        let home = tempfile::tempdir().unwrap();
        let native = "01a1a371-ecbd-7873-9451-a0afb83c99a1";
        let dir = home.path().join("sessions/2026/01/01");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("rollout-2026-01-01T00-00-00-{native}.jsonl"));
        let item = |ts: &str, payload: Value| json!({"timestamp": ts, "type": "response_item", "payload": payload});
        let user = |ts: &str, text: &str| {
            item(
                ts,
                json!({"type": "message", "role": "user",
                            "content": [{"type": "input_text", "text": text}]}),
            )
        };
        let lines = [
            json!({"timestamp": "2026-01-01T00:00:00Z", "type": "session_meta",
                   "payload": {"id": native, "cwd": "/r", "git": {"branch": "feat"}}}),
            user(
                "2026-01-01T00:00:00Z",
                "<environment_context>x</environment_context>",
            ),
            user("2026-01-01T00:00:01Z", "ship it"),
            item(
                "2026-01-01T00:00:02Z",
                json!({"type": "function_call", "name": "exec_command",
                                               "arguments": "{\"cmd\":\"git push\"}"}),
            ),
            item(
                "2026-01-01T00:00:03Z",
                json!({"type": "function_call_output", "output": "ok"}),
            ),
            user("2026-01-01T00:01:00Z", "no, never push without asking"),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&file, text).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_session (id TEXT, native_session_id TEXT, primary_model TEXT);
             INSERT INTO agent_session VALUES ('s_1', '01a1a371-ecbd-7873-9451-a0afb83c99a1', 'gpt-x');",
        )
        .unwrap();
        let rollouts = codex::rollout_index(home.path());
        let files = Files {
            projects: None,
            rollouts: &rollouts,
        };
        let mut mined = receipt("no, never push without asking", None);
        mined.agent = "codex".into();
        let case = case_for(&conn, &files, "l", &mined).unwrap().unwrap();
        assert_eq!(case.agent, Agent::Codex);
        assert_eq!(case.id, "s_1:5");
        assert_eq!(case.native, native);
        assert_eq!(case.file, file);
        assert_eq!((case.prompt_line, case.correction_line), (2, 5));
        assert_eq!(case.prompt, "ship it");
        assert_eq!(case.cwd, Some(PathBuf::from("/r")));
        assert_eq!(case.branch.as_deref(), Some("feat"));
        assert_eq!(case.model.as_deref(), Some("gpt-x"));
        assert_eq!(
            case.actions,
            vec![Action::Tool {
                name: "exec_command".into(),
                input: json!({"cmd": "git push"})
            }]
        );
        let json = serde_json::to_value(&case).unwrap();
        assert_eq!(json["agent"], "codex");

        mined.agent = "gemini".into();
        assert!(matches!(
            case_for(&conn, &files, "l", &mined).unwrap(),
            Err(Skip::UnsupportedAgent)
        ));
    }

    #[test]
    fn a_codex_sub_agent_rollout_is_skipped_by_reason() {
        let home = tempfile::tempdir().unwrap();
        let native = "01a1a220-7433-7310-95b5-72920ec9b737";
        let dir = home.path().join("sessions/2026/01/01");
        std::fs::create_dir_all(&dir).unwrap();
        let user = |ts: &str, text: &str| {
            json!({"timestamp": ts, "type": "response_item", "payload": {
                "type": "message", "role": "user",
                "content": [{"type": "input_text", "text": text}]}})
        };
        // What a sub-agent's rollout opens with: the parent spawned it.
        let meta = json!({"timestamp": "2026-01-01T00:00:00Z", "type": "session_meta",
            "payload": {"id": native, "cwd": "/r", "thread_source": "subagent",
                "multi_agent_version": "v2",
                "parent_thread_id": "01a19d41-05b4-7612-96d3-3c2ab1feba5e",
                "source": {"subagent": {"thread_spawn": {
                    "parent_thread_id": "01a19d41-05b4-7612-96d3-3c2ab1feba5e",
                    "depth": 1, "agent_role": "worker"}}}}});
        let lines = [
            meta,
            user("2026-01-01T00:00:01Z", "review the change"),
            user("2026-01-01T00:01:00Z", "no, never push without asking"),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(
            dir.join(format!("rollout-2026-01-01T00-00-00-{native}.jsonl")),
            text,
        )
        .unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE agent_session (id TEXT, native_session_id TEXT, primary_model TEXT);
             INSERT INTO agent_session VALUES ('s_1', '{native}', NULL);"
        ))
        .unwrap();
        let rollouts = codex::rollout_index(home.path());
        let files = Files {
            projects: None,
            rollouts: &rollouts,
        };
        let mut mined = receipt("no, never push without asking", None);
        mined.agent = "codex".into();
        let skip = case_for(&conn, &files, "l", &mined).unwrap().unwrap_err();
        assert_eq!(skip, Skip::CodexSubagent);

        let mut found = Extraction::default();
        found.skipped.insert(("codex".into(), skip), 1);
        assert_eq!(
            found.summary(),
            "cases 0 (claude-code 0, codex 0); skipped \
             codex sub-agent rollout, resumable only through its parent: 1"
        );
    }

    #[test]
    fn misses_carry_their_reason() {
        let entries = vec![human("only turn", "2026-01-01T00:00:00Z")];
        assert!(matches!(
            locate(&entries, &receipt("absent", None), &CLAUDE),
            Err(Skip::CorrectionLineNotFound)
        ));
        assert!(matches!(
            locate(&entries, &receipt("only turn", None), &CLAUDE),
            Err(Skip::NoPromptBeforeCorrection)
        ));
        assert!(matches!(
            locate(&entries, &receipt("   ", None), &CLAUDE),
            Err(Skip::EmptyCorrection)
        ));
    }

    #[test]
    fn locating_claude_prompt_skips_array_wrappers_and_blank_turns() {
        let entries = vec![
            human("fix the bug", "2026-01-01T00:00:00Z"),
            json!({"type": "user", "timestamp": "2026-01-01T00:00:01Z", "message": {
                "content": [{"type": "text", "text": "<command-name>/review</command-name>"}]}}),
            human("", "2026-01-01T00:00:02Z"),
            human("no, fix the test first", "2026-01-01T00:00:03Z"),
        ];
        let found = locate(&entries, &receipt("no, fix the test first", None), &CLAUDE)
            .ok()
            .unwrap();
        assert_eq!((found.prompt_line, found.correction_line), (0, 3));
        assert_eq!(found.prompt["message"]["content"], "fix the bug");
    }
}
