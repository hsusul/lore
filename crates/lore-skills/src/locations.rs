//! Where skills live and which agents load them. One table, so adding an agent
//! or a folder is one row, not a new branch in the scanner.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::model::Agent;

/// How skill folders sit under a location's directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Every child folder holding a SKILL.md is a skill.
    Flat,
    /// Skills sit in `skills/` folders somewhere below, as in
    /// `cache/<marketplace>/<plugin>/<version>/skills/<name>`.
    Nested,
}

/// How a location proves where its skills came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// `~/.agents/.skill-lock.json` and `~/.agents/skills/.opik-skills.json`.
    Lockfiles,
    /// `<claude>/plugins/installed_plugins.json`. Cached plugins it does not
    /// list are not loaded.
    ClaudePlugins,
    /// `<codex>/config.toml` `[plugins."<plugin>@<marketplace>"]`. Cached
    /// plugins it does not enable are not loaded.
    CodexPlugins,
    /// Shipped with the agent.
    Builtin(Agent),
    /// Nothing records it.
    None,
}

/// The root a location's directory is relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// The user's home.
    Home,
    /// Claude Code's config root: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    Claude,
    /// Codex's home: `$CODEX_HOME`, else `~/.codex`.
    Codex,
}

#[derive(Debug, Clone, Copy)]
pub struct Location {
    pub id: &'static str,
    pub base: Base,
    /// Relative to `base`.
    pub dir: &'static str,
    pub layout: Layout,
    /// Agents that load skills from here. Empty means the skills exist on the
    /// machine, so a reference to one is not a typo, but no agent loads them.
    pub agents: &'static [Agent],
    /// Agents whose fix commands link missing skills into this directory.
    pub primary_for: &'static [Agent],
    pub provenance: Provenance,
    /// The agent that writes this directory itself. A fix never edits, moves,
    /// or links into a managed copy: the agent replaces it on update.
    pub managed_by: Option<Agent>,
}

/// Every location, in the order a duplicate's copies are listed.
pub const LOCATIONS: &[Location] = &[
    Location {
        id: "agents",
        base: Base::Home,
        dir: ".agents/skills",
        layout: Layout::Flat,
        agents: &[Agent::Codex, Agent::Cursor],
        primary_for: &[Agent::Codex, Agent::Cursor],
        provenance: Provenance::Lockfiles,
        managed_by: None,
    },
    Location {
        id: "claude",
        base: Base::Claude,
        dir: "skills",
        layout: Layout::Flat,
        agents: &[Agent::ClaudeCode],
        primary_for: &[Agent::ClaudeCode],
        provenance: Provenance::Lockfiles,
        managed_by: None,
    },
    Location {
        id: "codex",
        base: Base::Codex,
        dir: "skills",
        layout: Layout::Flat,
        agents: &[Agent::Codex],
        primary_for: &[],
        provenance: Provenance::None,
        managed_by: None,
    },
    Location {
        id: "codex-system",
        base: Base::Codex,
        dir: "skills/.system",
        layout: Layout::Flat,
        agents: &[Agent::Codex],
        primary_for: &[],
        provenance: Provenance::Builtin(Agent::Codex),
        managed_by: Some(Agent::Codex),
    },
    Location {
        id: "cursor",
        base: Base::Home,
        dir: ".cursor/skills",
        layout: Layout::Flat,
        agents: &[Agent::Cursor],
        primary_for: &[],
        provenance: Provenance::None,
        managed_by: None,
    },
    Location {
        id: "claude-plugins",
        base: Base::Claude,
        dir: "plugins/cache",
        layout: Layout::Nested,
        agents: &[Agent::ClaudeCode],
        primary_for: &[],
        provenance: Provenance::ClaudePlugins,
        managed_by: Some(Agent::ClaudeCode),
    },
    Location {
        id: "codex-plugins",
        base: Base::Codex,
        dir: "plugins/cache",
        layout: Layout::Nested,
        agents: &[Agent::Codex],
        primary_for: &[],
        provenance: Provenance::CodexPlugins,
        managed_by: Some(Agent::Codex),
    },
    Location {
        id: "vendor",
        base: Base::Home,
        dir: ".agents/vendor",
        layout: Layout::Nested,
        agents: &[],
        primary_for: &[],
        provenance: Provenance::None,
        managed_by: None,
    },
];

/// The roots every location hangs from. Resolved the way the session adapters
/// resolve them, so `skills` and `scan` read the same agent folders.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Roots {
    pub home: PathBuf,
    pub claude: PathBuf,
    pub codex: PathBuf,
}

impl Roots {
    /// `~/.claude` and `~/.codex` under `home`.
    #[must_use]
    pub fn from_home(home: &Path) -> Self {
        Self::resolve(home, None, None)
    }

    /// The roots this process's environment names: `$HOME`, with
    /// `$CLAUDE_CONFIG_DIR` and `$CODEX_HOME` winning when set. `None` when
    /// `$HOME` is unset or not absolute.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())?;
        Some(Self::resolve(
            &home,
            std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
            std::env::var_os("CODEX_HOME").map(PathBuf::from),
        ))
    }

    /// `claude_config_dir` (`$CLAUDE_CONFIG_DIR`) and `codex_home`
    /// (`$CODEX_HOME`) win over the defaults under `home` when set.
    #[must_use]
    pub fn resolve(
        home: &Path,
        claude_config_dir: Option<PathBuf>,
        codex_home: Option<PathBuf>,
    ) -> Self {
        Self {
            home: home.to_path_buf(),
            claude: claude_config_dir.unwrap_or_else(|| home.join(".claude")),
            codex: codex_home.unwrap_or_else(|| home.join(".codex")),
        }
    }

    #[must_use]
    pub fn base(&self, base: Base) -> &Path {
        match base {
            Base::Home => &self.home,
            Base::Claude => &self.claude,
            Base::Codex => &self.codex,
        }
    }

    /// A location's directory under these roots.
    #[must_use]
    pub fn dir(&self, location: &Location) -> PathBuf {
        self.base(location.base).join(location.dir)
    }

    /// The directory where a fix links a skill for `agent`.
    #[must_use]
    pub fn primary_dir(&self, agent: Agent) -> PathBuf {
        LOCATIONS
            .iter()
            .find(|l| l.primary_for.contains(&agent))
            .map_or_else(|| self.home.join(".agents/skills"), |l| self.dir(l))
    }

    /// The agent that manages `path`, when it sits in a managed location
    /// itself or links into one. Resolving links only reads.
    #[must_use]
    pub fn manager(&self, path: &Path) -> Option<Agent> {
        let real = fs::canonicalize(path).ok();
        LOCATIONS.iter().find_map(|location| {
            let agent = location.managed_by?;
            let dir = self.dir(location);
            let real_dir = fs::canonicalize(&dir).ok();
            let inside = path.starts_with(&dir)
                || real.as_ref().is_some_and(|real| {
                    real.starts_with(&dir) || real_dir.as_ref().is_some_and(|d| real.starts_with(d))
                });
            inside.then_some(agent)
        })
    }
}

/// Position in [`LOCATIONS`], for ordering.
#[must_use]
pub fn order(id: &str) -> usize {
    LOCATIONS
        .iter()
        .position(|l| l.id == id)
        .unwrap_or(LOCATIONS.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_a_primary_directory_it_loads() {
        let roots = Roots::from_home(Path::new("/h"));
        for agent in [Agent::ClaudeCode, Agent::Codex, Agent::Cursor] {
            let dir = roots.primary_dir(agent);
            let loads = LOCATIONS.iter().any(|l| {
                roots.dir(l) == dir && l.agents.contains(&agent) && l.managed_by.is_none()
            });
            assert!(
                loads,
                "{agent} would be told to link into {}, which it does not load",
                dir.display()
            );
        }
    }

    #[test]
    fn location_ids_are_unique() {
        let mut ids: Vec<&str> = LOCATIONS.iter().map(|l| l.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), LOCATIONS.len());
    }

    #[test]
    fn the_overrides_move_the_claude_and_codex_roots_only() {
        let roots = Roots::resolve(
            Path::new("/h"),
            Some(PathBuf::from("/cfg")),
            Some(PathBuf::from("/cx")),
        );
        let dirs: Vec<PathBuf> = LOCATIONS.iter().map(|l| roots.dir(l)).collect();
        let want: Vec<PathBuf> = [
            "/h/.agents/skills",
            "/cfg/skills",
            "/cx/skills",
            "/cx/skills/.system",
            "/h/.cursor/skills",
            "/cfg/plugins/cache",
            "/cx/plugins/cache",
            "/h/.agents/vendor",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(dirs, want);
    }

    #[test]
    fn caches_and_codex_system_are_managed_and_user_folders_are_not() {
        let roots = Roots::from_home(Path::new("/h"));
        let manager = |p: &str| roots.manager(Path::new(p));
        assert_eq!(
            manager("/h/.claude/plugins/cache/m/p/1/skills/s"),
            Some(Agent::ClaudeCode)
        );
        assert_eq!(
            manager("/h/.codex/plugins/cache/m/p/1/skills/s"),
            Some(Agent::Codex)
        );
        assert_eq!(manager("/h/.codex/skills/.system/s"), Some(Agent::Codex));
        assert_eq!(manager("/h/.codex/skills/s"), None);
        assert_eq!(manager("/h/.claude/skills/s"), None);
        assert_eq!(manager("/h/.agents/vendor/pack/skills/s"), None);
    }
}
