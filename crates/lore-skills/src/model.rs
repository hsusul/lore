//! The domain: what a skill is, where it is installed, and where it came from.

use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

/// A skill's name: its folder name, which every agent uses to address it.
pub type SkillName = String;

/// A coding agent that loads skills from disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Agent {
    ClaudeCode,
    Codex,
    Cursor,
}

impl Agent {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
        }
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// blake3 over a skill folder's files, as lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ContentHash(pub String);

impl ContentHash {
    #[must_use]
    pub fn short(&self) -> &str {
        self.0.get(..12).unwrap_or(&self.0)
    }
}

/// How a skill folder sits in its location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Mode {
    Copy,
    Symlink { target: PathBuf },
}

/// Where a skill's content came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Source {
    Git {
        url: String,
        rev: String,
        subpath: String,
    },
    Local {
        path: PathBuf,
    },
    Builtin {
        agent: Agent,
    },
    Unknown,
}

impl Source {
    /// Higher is more specific. A record takes the most specific source any
    /// of its installs can prove.
    #[must_use]
    pub fn specificity(&self) -> u8 {
        match self {
            Source::Git { .. } => 4,
            Source::Builtin { .. } => 2,
            Source::Local { .. } => 1,
            Source::Unknown => 0,
        }
    }
}

/// One folder on disk holding a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Install {
    /// The [`crate::locations::Location`] id this folder was found under.
    pub location: &'static str,
    /// Every agent that loads this location.
    pub agents: Vec<Agent>,
    pub path: PathBuf,
    /// `path` with symlinks resolved. Two installs with one real path are one
    /// copy on disk.
    pub real_path: PathBuf,
    pub mode: Mode,
    pub hash: ContentHash,
}

/// One distinct content of one skill name. Identical copies share a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillRecord {
    pub name: SkillName,
    pub hash: ContentHash,
    pub source: Source,
    pub installs: Vec<Install>,
    /// Frontmatter `name`, when it parsed.
    pub declared_name: Option<String>,
    pub description: Option<String>,
    /// Why the frontmatter did not parse, when it did not.
    pub frontmatter_error: Option<String>,
    /// Line count of SKILL.md.
    pub lines: usize,
}
