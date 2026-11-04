//! The domain: what a skill is, where it is installed, where it came from, and
//! what can be wrong with it.

use std::collections::btree_map::{BTreeMap, Entry};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::locations::Roots;

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

    /// What a person can do about it. Suggested, never run. A command only
    /// reads, opens an editor, or links into a folder the user owns; it never
    /// edits, moves, deletes, or links to a copy an agent manages.
    #[must_use]
    pub fn fix(&self, roots: &Roots) -> Fix {
        match self {
            Finding::BadFrontmatter { path, .. } => {
                edit_or_report(roots, path, "the bad frontmatter")
            }
            Finding::NameMismatch { path, .. } => edit_or_report(roots, path, "the name mismatch"),
            Finding::BrokenRef {
                missing,
                agent,
                found_at,
                ..
            } => match roots.manager(found_at) {
                // A cache path carries a version, and a disabled plugin is off
                // on purpose: a link to either is wrong now or after an update.
                Some(manager) => Fix::Hint(format!(
                    "{agent} cannot load {}. A copy is at {}, managed by {manager}. \
                     Install it where {agent} loads skills. Do not link the managed \
                     copy, since its path changes on update.",
                    shell_quote(missing),
                    quote_path(found_at),
                )),
                // Guarded: a second `ln -s` onto a link to a folder would
                // put a link inside the user's own copy.
                None => {
                    let dest = quote_path(&roots.primary_dir(*agent).join(missing));
                    Fix::Command(format!(
                        "[ -e {dest} ] || ln -s {} {dest}",
                        quote_path(found_at)
                    ))
                }
            },
            Finding::Drift { copies, .. } => {
                let paths: Vec<String> =
                    copies.iter().take(2).map(|c| quote_path(&c.path)).collect();
                Fix::Command(format!("diff -ru {}", paths.join(" ")))
            }
            Finding::CodexBudget { largest, .. } => match roots.manager(largest) {
                Some(manager) => Fix::Hint(format!(
                    "The largest entry is {}, managed by {manager}. Disable the skills \
                     or plugins Codex does not need.",
                    quote_path(&largest.join("SKILL.md"))
                )),
                None => edit(largest),
            },
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

/// Each finding's [`Finding::fix`], in order, with at most one link command per
/// destination. Codex and Cursor load the same folder, so a skill one cannot
/// load is a broken ref for both; the later finding gets a hint pointing at
/// the command above it.
#[must_use]
pub fn fixes(findings: &[Finding], roots: &Roots) -> Vec<Fix> {
    let mut linked: BTreeMap<PathBuf, Agent> = BTreeMap::new();
    findings
        .iter()
        .map(|finding| {
            let fix = finding.fix(roots);
            let (Fix::Command(_), Finding::BrokenRef { missing, agent, .. }) = (&fix, finding)
            else {
                return fix;
            };
            let dir = roots.primary_dir(*agent);
            match linked.entry(dir.join(missing)) {
                Entry::Vacant(slot) => {
                    slot.insert(*agent);
                    fix
                }
                Entry::Occupied(first) if first.get() == agent => Fix::Hint(format!(
                    "The fix above already links {}.",
                    quote_path(first.key())
                )),
                Entry::Occupied(first) => Fix::Hint(format!(
                    "The fix for {} above covers {agent} too: both load skills from {}.",
                    first.get(),
                    quote_path(&dir)
                )),
            }
        })
        .collect()
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

/// Open a user-owned SKILL.md in the editor, or say whom to tell when an agent
/// manages it.
fn edit_or_report(roots: &Roots, path: &Path, what: &str) -> Fix {
    match roots.manager(path) {
        Some(manager) => Fix::Hint(format!(
            "Managed by {manager}. Report {what} in {} to its maintainer. Do not \
             edit the managed copy.",
            quote_path(&path.join("SKILL.md"))
        )),
        None => edit(path),
    }
}

/// `vi` when `EDITOR` is unset, so the path is never run as the command.
fn edit(path: &Path) -> Fix {
    Fix::Command(format!(
        "${{EDITOR:-vi}} {}",
        quote_path(&path.join("SKILL.md"))
    ))
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
    fn a_managed_copy_gets_a_hint_never_a_command() {
        let roots = Roots::from_home(Path::new("/h"));
        let cache = PathBuf::from("/h/.claude/plugins/cache/m/p/1.0/skills/s");
        let system = PathBuf::from("/h/.codex/skills/.system/s");
        for finding in [
            Finding::NameMismatch {
                skill: "s".into(),
                declared: "t".into(),
                path: cache.clone(),
            },
            Finding::BadFrontmatter {
                skill: "s".into(),
                path: system.clone(),
                reason: "r".into(),
            },
            Finding::BrokenRef {
                skill: "r".into(),
                missing: "s".into(),
                agent: Agent::Codex,
                path: PathBuf::from("/h/.agents/skills/r"),
                found_at: cache.clone(),
            },
            Finding::CodexBudget {
                chars: 9000,
                limit: 8000,
                skills: 1,
                largest: system,
            },
        ] {
            assert_eq!(finding.fix(&roots).kind(), "hint", "{finding:?}");
        }
        let fix = Finding::NameMismatch {
            skill: "s".into(),
            declared: "t".into(),
            path: cache,
        }
        .fix(&roots);
        assert_eq!(
            fix.text(),
            "Managed by claude-code. Report the name mismatch in \
             '/h/.claude/plugins/cache/m/p/1.0/skills/s/SKILL.md' to its maintainer. \
             Do not edit the managed copy."
        );
    }

    #[test]
    fn a_second_referrer_to_the_same_missing_skill_gets_a_hint() {
        let roots = Roots::from_home(Path::new("/h"));
        let broken = |skill: &str| Finding::BrokenRef {
            skill: skill.into(),
            missing: "s".into(),
            agent: Agent::Codex,
            path: PathBuf::from("/h/.agents/skills").join(skill),
            found_at: PathBuf::from("/h/.claude/skills/s"),
        };
        assert_eq!(
            fixes(&[broken("a"), broken("b")], &roots),
            vec![
                Fix::Command(
                    "[ -e '/h/.agents/skills/s' ] || ln -s '/h/.claude/skills/s' \
                     '/h/.agents/skills/s'"
                        .into()
                ),
                Fix::Hint("The fix above already links '/h/.agents/skills/s'.".into()),
            ]
        );
    }

    #[test]
    fn a_duplicate_with_three_copies_diffs_each_against_the_first() {
        let roots = Roots::from_home(Path::new("/h"));
        let fix = Finding::Duplicate {
            skill: "s".into(),
            paths: vec![
                "/h/a".into(),
                "/h/b".into(),
                "/h/.codex/plugins/cache/c".into(),
            ],
        }
        .fix(&roots);
        assert_eq!(
            fix,
            Fix::Command(
                "diff -ru '/h/a' '/h/b'; diff -ru '/h/a' '/h/.codex/plugins/cache/c'".into()
            )
        );
    }

    #[test]
    fn agent_name_and_display() {
        for (agent, name) in [
            (Agent::ClaudeCode, "claude-code"),
            (Agent::Codex, "codex"),
            (Agent::Cursor, "cursor"),
        ] {
            assert_eq!(agent.name(), name);
            assert_eq!(agent.to_string(), name);
        }
    }

    #[test]
    fn content_hash_short_prefix() {
        let full = ContentHash("0123456789abcdef".into());
        assert_eq!(full.short(), "0123456789ab");
        let short = ContentHash("short".into());
        assert_eq!(short.short(), "short");
    }

    #[test]
    fn source_specificity_ordering() {
        let git = Source::Git {
            url: "https://example.com/repo.git".into(),
            rev: "main".into(),
            subpath: "skills/test".into(),
        };
        let plugin = Source::Plugin {
            marketplace: "m".into(),
            plugin: "p".into(),
            version: "1.0".into(),
        };
        let builtin = Source::Builtin {
            agent: Agent::Codex,
        };
        let local = Source::Local {
            path: PathBuf::from("/path/to/skill"),
        };
        let unknown = Source::Unknown;

        assert_eq!(git.specificity(), 4);
        assert_eq!(plugin.specificity(), 3);
        assert_eq!(builtin.specificity(), 2);
        assert_eq!(local.specificity(), 1);
        assert_eq!(unknown.specificity(), 0);
    }

    #[test]
    fn finding_skill_and_kind_tags() {
        let bad_fm = Finding::BadFrontmatter {
            skill: "my-skill".into(),
            path: PathBuf::from("/path"),
            reason: "err".into(),
        };
        let budget = Finding::CodexBudget {
            chars: 1000,
            limit: 800,
            skills: 2,
            largest: PathBuf::from("/path"),
        };
        let unknown_src = Finding::UnknownSource {
            skill: "custom".into(),
            path: PathBuf::from("/path/custom"),
        };

        assert_eq!(bad_fm.skill(), Some("my-skill"));
        assert_eq!(bad_fm.kind(), "bad-frontmatter");

        assert_eq!(budget.skill(), None);
        assert_eq!(budget.kind(), "codex-budget");

        assert_eq!(unknown_src.skill(), Some("custom"));
        assert_eq!(unknown_src.kind(), "unknown-source");
    }

    #[test]
    fn fix_kind_and_text_accessors() {
        let cmd = Fix::Command("echo test".into());
        assert_eq!(cmd.kind(), "command");
        assert_eq!(cmd.text(), "echo test");

        let hint = Fix::Hint("check this".into());
        assert_eq!(hint.kind(), "hint");
        assert_eq!(hint.text(), "check this");
    }
}
