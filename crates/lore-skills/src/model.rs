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
    /// One name with two or more different contents.
    Drift {
        skill: SkillName,
        copies: Vec<DriftCopy>,
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
            Finding::Drift { .. } => 3,
            Finding::Duplicate { .. } => 5,
            Finding::UnknownSource { .. } => 6,
        }
    }

    /// The skill this finding is about.
    #[must_use]
    pub fn skill(&self) -> Option<&str> {
        match self {
            Finding::Drift { skill, .. }
            | Finding::Duplicate { skill, .. }
            | Finding::UnknownSource { skill, .. } => Some(skill),
        }
    }

    /// The kebab-case tag, as in `--json`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Finding::Drift { .. } => "drift",
            Finding::Duplicate { .. } => "duplicate",
            Finding::UnknownSource { .. } => "unknown-source",
        }
    }

    /// What a person can do about it. Suggested, never run. A command only
    /// reads; it never edits, moves, or deletes a copy.
    #[must_use]
    pub fn fix(&self) -> Fix {
        match self {
            Finding::Drift { copies, .. } => {
                let paths: Vec<String> =
                    copies.iter().take(2).map(|c| quote_path(&c.path)).collect();
                Fix::Command(format!("diff -ru {}", paths.join(" ")))
            }
            // Which copy to keep is the user's call, and a managed copy is not
            // theirs to move: show how the copies differ and stop there.
            Finding::Duplicate { paths, .. } => {
                let Some((first, rest)) = paths.split_first() else {
                    return Fix::Hint(String::new());
                };
                let diffs: Vec<String> = rest
                    .iter()
                    .map(|other| format!("diff -ru {} {}", quote_path(first), quote_path(other)))
                    .collect();
                Fix::Command(diffs.join("; "))
            }
            // Not a command: reinstalling over a hand-written skill loses it.
            Finding::UnknownSource { path, .. } => Fix::Hint(format!(
                "No lockfile or plugin records {}. If you wrote it, there is nothing \
                 to do. If it came from a repo, back up any local edits, then \
                 reinstall it with the skills CLI so a lockfile records it.",
                quote_path(path)
            )),
        }
    }
}

/// A [`Finding`]'s suggested fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "text", rename_all = "kebab-case")]
pub enum Fix {
    /// Safe to paste into a POSIX shell: every path and name in it went
    /// through [`shell_quote`].
    Command(String),
    /// Advice for a person. Not a command.
    Hint(String),
}

impl Fix {
    /// `command` or `hint`, as in `--json`'s `fix_kind`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Fix::Command(_) => "command",
            Fix::Hint(_) => "hint",
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Fix::Command(text) | Fix::Hint(text) => text,
        }
    }
}

/// The one quoting rule for every fix: wrap in single quotes, and write each
/// single quote as `'\''`. Nothing inside single quotes is special to a POSIX
/// shell, so `$`, backticks, `&`, `;`, and spaces stay literal.
#[must_use]
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn quote_path(path: &Path) -> String {
    shell_quote(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_keeps_metacharacters_literal() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("x'; touch y; '"), r"'x'\''; touch y; '\'''");
        assert_eq!(shell_quote("$(id)&`id`"), "'$(id)&`id`'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn a_duplicate_with_three_copies_diffs_each_against_the_first() {
        let fix = Finding::Duplicate {
            skill: "s".into(),
            paths: vec![
                "/h/a".into(),
                "/h/b".into(),
                "/h/.codex/plugins/cache/c".into(),
            ],
        }
        .fix();
        assert_eq!(
            fix,
            Fix::Command(
                "diff -ru '/h/a' '/h/b'; diff -ru '/h/a' '/h/.codex/plugins/cache/c'".into()
            )
        );
    }
}
