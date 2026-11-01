//! Where skills live and which agents load them. One table, so adding an agent
//! or a folder is one row, not a new branch in the scanner.

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
    /// `~/.claude/plugins/installed_plugins.json`. Cached plugins it does not
    /// list are not loaded.
    ClaudePlugins,
    /// `~/.codex/config.toml` `[plugins."<plugin>@<marketplace>"]`. Cached
    /// plugins it does not enable are not loaded.
    CodexPlugins,
    /// Shipped with the agent.
    Builtin(Agent),
    /// Nothing records it.
    None,
}

#[derive(Debug, Clone, Copy)]
pub struct Location {
    pub id: &'static str,
    /// Relative to the home root.
    pub dir: &'static str,
    pub layout: Layout,
    /// Agents that load skills from here. Empty means the skills exist on the
    /// machine, so a reference to one is not a typo, but no agent loads them.
    pub agents: &'static [Agent],
    /// Agents whose fix commands link missing skills into this directory.
    pub primary_for: &'static [Agent],
    pub provenance: Provenance,
}

/// Every location, in the order a duplicate's copy to keep is chosen.
pub const LOCATIONS: &[Location] = &[
    Location {
        id: "agents",
        dir: ".agents/skills",
        layout: Layout::Flat,
        agents: &[Agent::Codex, Agent::Cursor],
        primary_for: &[Agent::Codex, Agent::Cursor],
        provenance: Provenance::Lockfiles,
    },
    Location {
        id: "claude",
        dir: ".claude/skills",
        layout: Layout::Flat,
        agents: &[Agent::ClaudeCode],
        primary_for: &[Agent::ClaudeCode],
        provenance: Provenance::Lockfiles,
    },
    Location {
        id: "codex",
        dir: ".codex/skills",
        layout: Layout::Flat,
        agents: &[Agent::Codex],
        primary_for: &[],
        provenance: Provenance::None,
    },
    Location {
        id: "codex-system",
        dir: ".codex/skills/.system",
        layout: Layout::Flat,
        agents: &[Agent::Codex],
        primary_for: &[],
        provenance: Provenance::Builtin(Agent::Codex),
    },
    Location {
        id: "cursor",
        dir: ".cursor/skills",
        layout: Layout::Flat,
        agents: &[Agent::Cursor],
        primary_for: &[],
        provenance: Provenance::None,
    },
    Location {
        id: "claude-plugins",
        dir: ".claude/plugins/cache",
        layout: Layout::Nested,
        agents: &[Agent::ClaudeCode],
        primary_for: &[],
        provenance: Provenance::ClaudePlugins,
    },
    Location {
        id: "codex-plugins",
        dir: ".codex/plugins/cache",
        layout: Layout::Nested,
        agents: &[Agent::Codex],
        primary_for: &[],
        provenance: Provenance::CodexPlugins,
    },
    Location {
        id: "vendor",
        dir: ".agents/vendor",
        layout: Layout::Nested,
        agents: &[],
        primary_for: &[],
        provenance: Provenance::None,
    },
];

/// The directory, relative to home, where a fix links a skill for `agent`.
#[must_use]
pub fn primary_dir(agent: Agent) -> &'static str {
    LOCATIONS
        .iter()
        .find(|l| l.primary_for.contains(&agent))
        .map_or(".agents/skills", |l| l.dir)
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
        for agent in [Agent::ClaudeCode, Agent::Codex, Agent::Cursor] {
            let dir = primary_dir(agent);
            let loads = LOCATIONS
                .iter()
                .any(|l| l.dir == dir && l.agents.contains(&agent));
            assert!(
                loads,
                "{agent} would be told to link into {dir}, which it does not load"
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
}
