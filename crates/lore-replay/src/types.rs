//! The data a replay reads and writes.
//!
//! Field names match the Python prototype's `cases.jsonl`, `arms.json`, and
//! `run.json`, so one report tool reads output from either.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use regex::Regex;
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

/// The current replay harness. 1 put the arm's context in the system prompt
/// and gave every run its own scratch path; 2 shares one scratch path per case
/// and delivers the context in the final user message, so every run of a case
/// shares a prompt-cache prefix. Records without the field are 1.
pub const HARNESS: u32 = 2;

/// How harness 2 delivers an arm's context: a `<system-reminder>` block
/// before the prompt, in the final user message.
pub const DELIVERY: &str = "user-message";

fn harness_one() -> u32 {
    1
}

/// `run.json`: one replay of one case under one arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    /// See [`HARNESS`].
    #[serde(default = "harness_one")]
    pub harness: u32,
    /// See [`DELIVERY`]; `None` in harness 1 records (the system prompt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
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

/// How a run is checked for a repeat of the labeled mistake. A closed set:
/// the labeling judge picks one, and anything it writes that is not one of
/// the deterministic kinds, or whose regex does not compile, is a [`Judge`].
///
/// [`Judge`]: Grader::Judge
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "RawGrader", into = "RawGrader")]
pub enum Grader {
    /// The final message asks the person whether or which way to proceed.
    AsksPermission,
    /// Any tool call, or refused `gh` call, matches.
    ToolPattern(Regex),
    /// Any written or edited file path matches.
    FileWritten(Regex),
    /// The final message matches.
    TextPattern(Regex),
    /// Only the model judge can tell; `rubric` is its hint (may be empty).
    Judge { rubric: String },
}

impl Grader {
    /// The prototype's `kind` string.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AsksPermission => "asks_permission",
            Self::ToolPattern(_) => "tool_pattern",
            Self::FileWritten(_) => "file_written",
            Self::TextPattern(_) => "text_pattern",
            Self::Judge { .. } => "judge",
        }
    }

    /// The judge's hint: the rubric, when this is a judge grader with one.
    #[must_use]
    pub fn rubric(&self) -> Option<&str> {
        match self {
            Self::Judge { rubric } if !rubric.trim().is_empty() => Some(rubric),
            _ => None,
        }
    }
}

impl PartialEq for Grader {
    fn eq(&self, other: &Self) -> bool {
        RawGrader::from(self.clone()) == RawGrader::from(other.clone())
    }
}

/// A grader as JSON: `{"kind", "pattern"}` or `{"kind": "judge", "rubric"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawGrader {
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric: Option<String>,
}

impl From<RawGrader> for Grader {
    /// Never fails: a bad or missing pattern becomes a judge grader, with the
    /// pattern kept as the judge's hint.
    fn from(raw: RawGrader) -> Self {
        let regex = |make: fn(Regex) -> Self| match raw.pattern.as_deref() {
            Some(p) => match Regex::new(p) {
                Ok(re) => make(re),
                Err(_) => Self::Judge {
                    rubric: format!("{} pattern that is not a valid regex here: {p}", raw.kind),
                },
            },
            None => Self::Judge {
                rubric: raw.rubric.clone().unwrap_or_default(),
            },
        };
        match raw.kind.as_str() {
            "asks_permission" => Self::AsksPermission,
            "tool_pattern" => regex(Self::ToolPattern),
            "file_written" => regex(Self::FileWritten),
            "text_pattern" => regex(Self::TextPattern),
            _ => Self::Judge {
                rubric: raw.rubric.clone().unwrap_or_default(),
            },
        }
    }
}

impl From<Grader> for RawGrader {
    fn from(grader: Grader) -> Self {
        let kind = grader.kind().to_owned();
        let (pattern, rubric) = match grader {
            Grader::AsksPermission => (None, None),
            Grader::ToolPattern(re) | Grader::FileWritten(re) | Grader::TextPattern(re) => {
                (Some(re.as_str().to_owned()), None)
            }
            Grader::Judge { rubric } => (None, Some(rubric)),
        };
        Self {
            kind,
            pattern,
            rubric,
        }
    }
}

/// The labeling judge's reading of one case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Label {
    /// The case id, as the judge echoed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The corrected turn did something the correction rejects.
    #[serde(default, deserialize_with = "null_default")]
    pub mistake: bool,
    /// One sentence naming the mistake.
    #[serde(default, deserialize_with = "null_default")]
    pub what: String,
    /// One imperative sentence the agent should have followed.
    #[serde(default, deserialize_with = "null_default")]
    pub lesson: String,
    #[serde(default)]
    pub grader: Option<Grader>,
    /// The mistake shows in the turn's actions, not only in the correction.
    #[serde(default, deserialize_with = "null_default")]
    pub visible: bool,
}

fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A `labeled.jsonl` line: the case as written, plus `label` and
/// `labeled_by` (the judge model). Other case fields pass through untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabeledCase {
    pub id: String,
    #[serde(default)]
    pub correction: String,
    #[serde(default)]
    pub label: Option<Label>,
    #[serde(default)]
    pub labeled_by: Option<String>,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

impl LabeledCase {
    /// The label, when it says the turn made a mistake visible in its actions:
    /// the only cases the analysis includes.
    #[must_use]
    pub fn included(&self) -> Option<&Label> {
        self.label.as_ref().filter(|l| l.mistake && l.visible)
    }
}

/// A judge's one-word answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Verdict {
    Yes,
    No,
    Unknown,
}

impl Verdict {
    /// YES is true, NO is false, UNKNOWN is no answer.
    #[must_use]
    pub fn as_bool(self) -> Option<bool> {
        match self {
            Self::Yes => Some(true),
            Self::No => Some(false),
            Self::Unknown => None,
        }
    }
}

/// A `grades.jsonl` line: one graded run. Same fields as the prototype's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradeLine {
    pub case: String,
    pub arm: ArmName,
    pub trial: u32,
    /// The label's grader kind.
    #[serde(default)]
    pub grader: Option<String>,
    /// The deterministic verdict; `None` for a judge-only grader.
    #[serde(default)]
    pub repeat_det: Option<bool>,
    /// The model judge's verdict; `None` when the judge was not asked.
    #[serde(default)]
    pub judge: Option<Verdict>,
    #[serde(default)]
    pub judge_reason: Option<String>,
    #[serde(default)]
    pub fidelity: Option<String>,
    #[serde(default)]
    pub foreign_paths: Vec<String>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub graded_by: Option<String>,
    /// The run's harness version (see [`HARNESS`]); `None` reads as 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<u32>,
}

impl GradeLine {
    /// The harness version, 1 when the line does not say.
    #[must_use]
    pub fn harness_version(&self) -> u32 {
        self.harness.unwrap_or(1)
    }
}

impl GradeLine {
    /// The verdict the analysis uses: deterministic when there is one, else
    /// the judge's. `None` (UNKNOWN or not judged) is excluded and counted.
    #[must_use]
    pub fn repeat(&self) -> Option<bool> {
        self.repeat_det
            .or_else(|| self.judge.and_then(Verdict::as_bool))
    }
}

/// A `shown.jsonl` line: does an arm's context for a case hold a lesson that
/// addresses the case's mistake.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShownLine {
    pub case: String,
    pub arm: ArmName,
    pub shown: Verdict,
    pub reason: String,
    pub judged_by: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graders_parse_at_the_boundary() {
        let parse = |s: &str| serde_json::from_str::<Grader>(s).unwrap();
        assert_eq!(
            parse(r#"{"kind":"asks_permission"}"#).kind(),
            "asks_permission"
        );
        let tool = parse(r#"{"kind":"tool_pattern","pattern":"gh pr create"}"#);
        assert!(matches!(&tool, Grader::ToolPattern(re) if re.is_match("x gh pr create")));
        let bad = parse(r#"{"kind":"file_written","pattern":"(unclosed"}"#);
        assert_eq!(bad.kind(), "judge");
        assert!(bad.rubric().unwrap().contains("(unclosed"));
        assert_eq!(parse(r#"{"kind":"text_pattern"}"#).kind(), "judge");
        assert_eq!(
            parse(r#"{"kind":"vibes","rubric":"r"}"#).rubric(),
            Some("r")
        );
        let round = serde_json::to_string(&tool).unwrap();
        assert_eq!(round, r#"{"kind":"tool_pattern","pattern":"gh pr create"}"#);
    }

    #[test]
    fn a_labeled_line_keeps_its_case_fields() {
        let line = r#"{"id":"s:1","correction":"no","prompt":"p","label":{"id":"s:1","mistake":true,"what":"w","lesson":null,"grader":null,"visible":true},"labeled_by":"judge: claude sonnet"}"#;
        let case: LabeledCase = serde_json::from_str(line).unwrap();
        assert!(case.included().is_some());
        assert_eq!(case.rest.get("prompt"), Some(&Value::from("p")));
        let back: Value = serde_json::to_value(&case).unwrap();
        assert_eq!(back["prompt"], "p");
        assert_eq!(back["labeled_by"], "judge: claude sonnet");
    }

    #[test]
    fn the_deterministic_verdict_wins_and_unknown_is_none() {
        let mut g: GradeLine = serde_json::from_str(
            r#"{"case":"c","arm":"none","trial":1,"repeat_det":false,"judge":"YES"}"#,
        )
        .unwrap();
        assert_eq!(g.repeat(), Some(false));
        g.repeat_det = None;
        assert_eq!(g.repeat(), Some(true));
        g.judge = Some(Verdict::Unknown);
        assert_eq!(g.repeat(), None);
    }

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
