//! `lore-replay`: replay a Claude Code or Codex session from the point a
//! person corrected the agent, to measure whether lessons stop the agent from
//! repeating the mistake.
//!
//! **This crate starts an agent that talks to a model provider.** `run`
//! launches Claude Code (`claude -p`) or Codex (`codex exec`), whichever
//! recorded the case, which sends the session prefix and the prompt to the
//! agent's provider (Anthropic or OpenAI) under the person's own login, and
//! is billed to it. Unlike lore-core, lore-memory, and lorectl, this crate is not
//! network-incapable by design, and nothing in the archive depends on it. It
//! holds no networking code itself; the agent it starts does.
//!
//! `cases` only reads: the archive, opened read only, and the agents'
//! session files. `label` and `grade` ask a model judge through `claude -p`
//! (also under the person's login); `grade --no-judge` and `report` only read.
//!
//! Each replay is isolated so that it measures the agent and not a leak:
//! a scratch clone at the commit HEAD pointed at when the prompt was given,
//! an OS sandbox with no network and no reads of `/Users` or `/Volumes`, a
//! session prefix rewritten into the clone, and a project folder (or, for
//! Codex, a `CODEX_HOME`) Lore never ingests. See [`isolation`] and
//! [`codex`] for why each rule exists.

pub mod cases;
pub mod claude;
pub mod codex;
pub mod error;
pub mod grade;
pub mod isolation;
pub mod label;
pub mod prefix;
pub mod pyjson;
pub mod report;
pub mod run;
pub mod scratch;
pub mod session;
pub mod sweep;
pub mod types;

use std::path::Path;

pub use error::{Error, Result};
pub use types::{
    Action, Agent, Arm, ArmName, ArmsFile, Case, Fidelity, GradeLine, Grader, Label, LabeledCase,
    RunRecord, ScreenLine, ShownLine, Verdict, DELIVERY, HARNESS,
};

/// The case `id` from a `cases.jsonl` file. Blank lines are allowed; any
/// other line must be a case.
pub fn find_case(cases: &Path, id: &str) -> Result<Case> {
    let text = std::fs::read_to_string(cases).map_err(|e| Error::io("read", cases, e))?;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let case: Case = serde_json::from_str(line).map_err(|source| Error::Json {
            path: cases.to_owned(),
            source,
        })?;
        if case.id == id {
            return Ok(case);
        }
    }
    Err(Error::CaseNotFound(id.to_owned()))
}

/// The arm `name` for `case`, its text read from `arms` when it needs one.
/// A caller-supplied arm with no text for the case is an error, so a `lore`
/// run never quietly becomes a `none` run.
pub fn load_arm(arms: Option<&Path>, case: &str, name: ArmName) -> Result<Arm> {
    let missing = || Error::ArmTextMissing {
        case: case.to_owned(),
        arm: name.to_string(),
    };
    if !name.needs_text() {
        return Arm::new(name, None).ok_or_else(missing);
    }
    let path = arms.ok_or_else(missing)?;
    let text = std::fs::read_to_string(path).map_err(|e| Error::io("read", path, e))?;
    let file: ArmsFile = serde_json::from_str(&text).map_err(|source| Error::Json {
        path: path.to_owned(),
        source,
    })?;
    let text = file
        .get(case)
        .and_then(|arms| arms.get(name.as_str()))
        .cloned();
    Arm::new(name, text).ok_or_else(missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_arm_rejects_empty_or_whitespace_text() {
        let dir = tempfile::tempdir().unwrap();
        let arms = dir.path().join("arms.json");
        std::fs::write(
            &arms,
            serde_json::json!({
                "c:0": {
                    "lore": "",
                    "file": "   \t\n  ",
                    "oracle": "context"
                }
            })
            .to_string(),
        )
        .unwrap();

        assert!(matches!(
            load_arm(Some(&arms), "c:0", ArmName::Lore),
            Err(Error::ArmTextMissing { case, arm }) if case == "c:0" && arm == "lore"
        ));
        assert!(matches!(
            load_arm(Some(&arms), "c:0", ArmName::File),
            Err(Error::ArmTextMissing { case, arm }) if case == "c:0" && arm == "file"
        ));
        assert!(matches!(
            load_arm(Some(&arms), "c:0", ArmName::Oracle),
            Ok(Arm::Oracle(t)) if t == "context"
        ));
    }
}
