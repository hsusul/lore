//! The data a replay reads and writes.
//!
//! Field names match the Python prototype's `cases.jsonl`, `arms.json`, and
//! `run.json`, so one report tool reads output from either.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// One step of the assistant turn a person corrected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Action {
    /// Prose the assistant wrote.
    Text { text: String },
    /// A tool call, with its input exactly as the session recorded it.
    Tool { name: String, input: Value },
}

/// Why a mined user turn is a candidate, as `lorectl mem mine --json` says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Signal {
    Correction,
    Repeat,
    Rule,
}

impl From<lore_memory::mine::Signal> for Signal {
    fn from(signal: lore_memory::mine::Signal) -> Self {
        match signal {
            lore_memory::mine::Signal::Correction => Self::Correction,
            lore_memory::mine::Signal::Repeat => Self::Repeat,
            lore_memory::mine::Signal::Rule => Self::Rule,
        }
    }
}

/// A correction point: the human turn that prompted an assistant turn, that
/// assistant turn, and the human turn that corrected it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    /// `<archive session id>:<correction line>`, stable across extractions.
    pub id: String,
    /// The mined lesson this correction is a receipt of.
    pub lesson: String,
    /// The archive session id.
    pub session: String,
    /// Claude Code's own session id: the session file's stem.
    pub native: String,
    /// The model the original session ran, when the archive knows it.
    pub model: Option<String>,
    /// The Claude Code session file.
    pub file: PathBuf,
    /// Zero-based line of the prompting human turn; the replay prefix is every
    /// line before it.
    pub prompt_line: usize,
    /// Zero-based line of the correcting human turn.
    pub correction_line: usize,
    pub correction_ms: i64,
    /// Where the prompt was given. `None` when the session line did not say.
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub prompt_ms: i64,
    pub prompt: String,
    /// The corrected assistant turn, between the prompt and the correction.
    pub actions: Vec<Action>,
    pub correction: String,
    /// The repository's display name, when the archive linked one.
    pub repo: Option<String>,
    pub signal: Signal,
}

/// Which context a replay adds to the agent's system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArmName {
    /// Nothing: the agent's base behavior.
    None,
    /// The person's own CLAUDE.md files, read when the replay runs.
    Own,
    /// A lessons file, supplied by the caller.
    File,
    /// What Lore would give the agent, supplied by the caller.
    Lore,
    /// The correction itself restated as a lesson, supplied by the caller.
    Oracle,
}

impl ArmName {
    pub const ALL: [Self; 5] = [Self::None, Self::Own, Self::File, Self::Lore, Self::Oracle];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Own => "own",
            Self::File => "file",
            Self::Lore => "lore",
            Self::Oracle => "oracle",
        }
    }

    /// True when the caller must supply the arm's text in the arms file.
    #[must_use]
    pub fn needs_text(self) -> bool {
        matches!(self, Self::File | Self::Lore | Self::Oracle)
    }
}

impl fmt::Display for ArmName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ArmName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|arm| arm.as_str() == s)
            .ok_or_else(|| format!("unknown arm `{s}`; expected none, own, file, lore, or oracle"))
    }
}

impl Serialize for ArmName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ArmName {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// An arm with its context. The caller-supplied arms carry their text, so a
/// `lore` arm without text cannot be built and silently run as `none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arm {
    None,
    Own,
    File(String),
    Lore(String),
    Oracle(String),
}

impl Arm {
    #[must_use]
    pub fn name(&self) -> ArmName {
        match self {
            Self::None => ArmName::None,
            Self::Own => ArmName::Own,
            Self::File(_) => ArmName::File,
            Self::Lore(_) => ArmName::Lore,
            Self::Oracle(_) => ArmName::Oracle,
        }
    }

    /// The arm for `name`, taking its text from `text` when it needs one.
    /// `None` when a caller-supplied arm has no text.
    #[must_use]
    pub fn new(name: ArmName, text: Option<String>) -> Option<Self> {
        Some(match name {
            ArmName::None => Self::None,
            ArmName::Own => Self::Own,
            ArmName::File => Self::File(text?),
            ArmName::Lore => Self::Lore(text?),
            ArmName::Oracle => Self::Oracle(text?),
        })
    }
}

/// `arms.json`: case id, then arm name, then the text to append.
pub type ArmsFile = std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>;

/// How faithfully the scratch repository reproduces the one the person had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fidelity {
    /// The commit HEAD pointed at when the prompt was given, without the
    /// uncommitted changes the working tree may have held.
    CommitOnly { commit: String },
    /// The session line names no working directory.
    NoCwd,
    /// The working directory no longer exists.
    CwdGone,
    /// The working directory is not inside a git repository.
    NotARepo,
    /// The reflog starts after the prompt, so HEAD at that moment is unknown.
    ReflogTooYoung,
}

impl fmt::Display for Fidelity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommitOnly { commit } => {
                let short = commit.get(..12).unwrap_or(commit);
                write!(f, "commit-only:{short}")
            }
            Self::NoCwd => f.write_str("no-cwd"),
            Self::CwdGone => f.write_str("cwd-gone"),
            Self::NotARepo => f.write_str("not-a-repo"),
            Self::ReflogTooYoung => f.write_str("reflog-too-young"),
        }
    }
}

impl FromStr for Fidelity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "no-cwd" => Self::NoCwd,
            "cwd-gone" => Self::CwdGone,
            "not-a-repo" => Self::NotARepo,
            "reflog-too-young" => Self::ReflogTooYoung,
            other => match other.strip_prefix("commit-only:") {
                Some(commit) if !commit.is_empty() => Self::CommitOnly {
                    commit: commit.to_owned(),
                },
                _ => return Err(format!("unknown fidelity `{other}`")),
            },
        })
    }
}

impl Serialize for Fidelity {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Fidelity {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// `run.json`: one replay of one case under one arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub case: String,
    pub arm: ArmName,
    pub trial: u32,
    pub model: String,
    pub fidelity: Fidelity,
    /// Paths the corrected turn touched outside the repository. The sandbox
    /// hides them, so a replay that needs them cannot reproduce the turn.
    pub foreign_paths: Vec<String>,
    pub prefix_lines: usize,
    pub seconds: f64,
    pub cost_usd: Option<f64>,
    pub is_error: Option<bool>,
    /// The last characters of the agent's stderr.
    pub stderr: String,
    /// Each attempt to reach GitHub through `gh`, which the stub refuses.
    pub outward: Vec<String>,
    /// The agent's stream-json events, in order.
    pub events: Vec<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fidelity_round_trips_through_the_prototype_strings() {
        let commit = Fidelity::CommitOnly {
            commit: "0123456789abcdef0123".into(),
        };
        assert_eq!(commit.to_string(), "commit-only:0123456789ab");
        for s in ["no-cwd", "cwd-gone", "not-a-repo", "reflog-too-young"] {
            assert_eq!(s.parse::<Fidelity>().unwrap().to_string(), s);
        }
        assert!("commit-only:".parse::<Fidelity>().is_err());
    }

    #[test]
    fn actions_use_the_prototype_kind_tag() {
        let json = r#"[{"kind":"text","text":"hi"},{"kind":"tool","name":"Bash","input":{"command":"ls"}}]"#;
        let actions: Vec<Action> = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&actions).unwrap(), json);
    }

    #[test]
    fn a_caller_supplied_arm_needs_its_text() {
        assert_eq!(Arm::new(ArmName::Lore, None), None);
        assert_eq!(Arm::new(ArmName::None, None), Some(Arm::None));
        assert_eq!(
            Arm::new(ArmName::Oracle, Some("x".into())).map(|a| a.name()),
            Some(ArmName::Oracle)
        );
    }
}
