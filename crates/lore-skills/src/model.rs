//! The domain: what a skill is, where it is installed, where it came from, and
//! what can be wrong with it.

use std::fmt;
use std::path::{Path, PathBuf};

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
    Plugin {
        marketplace: String,
        plugin: String,
        version: String,
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
            Source::Plugin { .. } => 3,
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
    /// Other skills this one's SKILL.md body names, limited to names that
    /// exist somewhere on the machine.
    pub refs: Vec<SkillName>,
    /// Frontmatter `name`, when it parsed.
    pub declared_name: Option<String>,
    pub description: Option<String>,
    /// Why the frontmatter did not parse, when it did not.
    pub frontmatter_error: Option<String>,
    /// Line count of SKILL.md.
    pub lines: usize,
}

/// One copy in a [`Finding::Drift`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DriftCopy {
    pub path: PathBuf,
    pub hash: ContentHash,
    pub lines: usize,
}

/// One problem the doctor reports. Variants are declared worst first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Finding {
    /// SKILL.md frontmatter that an agent's YAML reader would reject.
    BadFrontmatter {
        skill: SkillName,
        path: PathBuf,
        reason: String,
    },
    /// Frontmatter `name` differs from the folder name.
    NameMismatch {
        skill: SkillName,
        declared: String,
        path: PathBuf,
    },
    /// A skill names another skill that exists on this machine but is not
    /// reachable from an agent that loads the referencing skill.
    BrokenRef {
        skill: SkillName,
        missing: SkillName,
        agent: Agent,
        path: PathBuf,
        found_at: PathBuf,
    },
    /// One name with two or more different contents.
    Drift {
        skill: SkillName,
        copies: Vec<DriftCopy>,
    },
    /// Codex's summed skill names and descriptions exceed its budget.
    CodexBudget {
        chars: usize,
        limit: usize,
        skills: usize,
        largest: PathBuf,
    },
    /// One name in two or more locations as separate copies on disk.
    Duplicate {
        skill: SkillName,
        paths: Vec<PathBuf>,
    },
    /// No lockfile, plugin manifest, or symlink explains where it came from.
    UnknownSource { skill: SkillName, path: PathBuf },
}

impl Finding {
    /// Sort key, worst first.
    #[must_use]
    pub fn rank(&self) -> u8 {
        match self {
            Finding::BadFrontmatter { .. } => 0,
            Finding::NameMismatch { .. } => 1,
            Finding::BrokenRef { .. } => 2,
            Finding::Drift { .. } => 3,
            Finding::CodexBudget { .. } => 4,
            Finding::Duplicate { .. } => 5,
            Finding::UnknownSource { .. } => 6,
        }
    }

    /// The skill this finding is about. `CodexBudget` is about a whole agent.
    #[must_use]
    pub fn skill(&self) -> Option<&str> {
        match self {
            Finding::BadFrontmatter { skill, .. }
            | Finding::NameMismatch { skill, .. }
            | Finding::BrokenRef { skill, .. }
            | Finding::Drift { skill, .. }
            | Finding::Duplicate { skill, .. }
            | Finding::UnknownSource { skill, .. } => Some(skill),
            Finding::CodexBudget { .. } => None,
        }
    }

    /// The kebab-case tag, as in `--json`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Finding::BadFrontmatter { .. } => "bad-frontmatter",
            Finding::NameMismatch { .. } => "name-mismatch",
            Finding::BrokenRef { .. } => "broken-ref",
            Finding::Drift { .. } => "drift",
            Finding::CodexBudget { .. } => "codex-budget",
            Finding::Duplicate { .. } => "duplicate",
            Finding::UnknownSource { .. } => "unknown-source",
        }
    }

    /// One shell command that fixes the problem. Suggested, never run.
    #[must_use]
    pub fn fix(&self) -> String {
        match self {
            Finding::BadFrontmatter { path, .. } | Finding::CodexBudget { largest: path, .. } => {
                format!("$EDITOR {}", quote(&path.join("SKILL.md")))
            }
            Finding::NameMismatch { skill, path, .. } => format!(
                "sed -i.bak 's/^name:.*/name: {skill}/' {}",
                quote(&path.join("SKILL.md"))
            ),
            Finding::BrokenRef {
                missing,
                agent,
                found_at,
                ..
            } => format!(
                "ln -s {} ~/{}/{missing}",
                quote(found_at),
                crate::locations::primary_dir(*agent)
            ),
            Finding::Drift { copies, .. } => {
                let paths: Vec<String> = copies.iter().take(2).map(|c| quote(&c.path)).collect();
                format!("diff -ru {}", paths.join(" "))
            }
            // Moved aside rather than deleted: a duplicate can also be drifted,
            // and the copy being replaced may hold the only version of an edit.
            Finding::Duplicate { paths, .. } => {
                let Some((keep, rest)) = paths.split_first() else {
                    return String::new();
                };
                let steps: Vec<String> = rest
                    .iter()
                    .map(|other| {
                        format!(
                            "mv {o} \"$(mktemp -d)\"/ && ln -s {k} {o}",
                            o = quote(other),
                            k = quote(keep)
                        )
                    })
                    .collect();
                steps.join(" && ")
            }
            Finding::UnknownSource { skill, .. } => {
                format!("npx skills add <owner/repo> --skill {skill}")
            }
        }
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}
