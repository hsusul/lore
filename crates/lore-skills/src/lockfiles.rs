//! The files that record where skills came from. Every reader is lenient: a
//! missing or malformed file means "nothing recorded", never an error, because
//! the doctor reports on a machine as it is.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::locations::Roots;
use crate::model::{SkillName, Source};

/// Relative to the home root.
pub const SKILL_LOCK: &str = ".agents/.skill-lock.json";
/// Relative to the home root.
pub const OPIK_SKILLS: &str = ".agents/skills/.opik-skills.json";
/// Relative to Claude Code's config root.
pub const CLAUDE_PLUGINS: &str = "plugins/installed_plugins.json";
/// Relative to Claude Code's config root.
pub const CLAUDE_PLUGIN_CACHE: &str = "plugins/cache";
/// Relative to Codex's home.
pub const CODEX_CONFIG: &str = "config.toml";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lockfiles {
    /// Sources for skills installed into `~/.agents/skills`, by name.
    pub tracked: BTreeMap<SkillName, Source>,
    /// Claude Code plugin install directories, relative to the plugin cache,
    /// and the plugin each holds.
    pub claude_plugins: Vec<(PathBuf, Source)>,
    /// `<plugin>@<marketplace>` keys Codex has enabled.
    pub codex_enabled: BTreeSet<String>,
}

#[must_use]
pub fn read(roots: &Roots) -> Lockfiles {
    let home = &roots.home;
    let mut tracked = BTreeMap::new();
    // The skills CLI lock wins over the opik manifest if both name a skill:
    // it records a per-skill folder hash, the opik manifest does not.
    for (name, source) in opik(home).into_iter().chain(skill_lock(home)) {
        tracked.insert(name, source);
    }
    Lockfiles {
        tracked,
        claude_plugins: claude_plugins(&roots.claude),
        codex_enabled: codex_enabled(&roots.codex),
    }
}

fn json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `~/.agents/.skill-lock.json`, version 3 of vercel-labs/skills.
fn skill_lock(home: &Path) -> Vec<(SkillName, Source)> {
    let Some(lock) = json(&home.join(SKILL_LOCK)) else {
        return Vec::new();
    };
    let Some(skills) = lock.get("skills").and_then(Value::as_object) else {
        return Vec::new();
    };
    skills
        .iter()
        .map(|(name, entry)| {
            let url = match text(entry, "sourceUrl") {
                url if url.is_empty() => text(entry, "source"),
                url => url,
            };
            let source = Source::Git {
                url,
                rev: text(entry, "skillFolderHash"),
                subpath: text(entry, "skillPath"),
            };
            (name.clone(), source)
        })
        .collect()
}

/// `~/.agents/skills/.opik-skills.json`: one source and ref for a list of names.
fn opik(home: &Path) -> Vec<(SkillName, Source)> {
    let Some(manifest) = json(&home.join(OPIK_SKILLS)) else {
        return Vec::new();
    };
    let url = text(&manifest, "source");
    let rev = text(&manifest, "ref");
    manifest
        .get("skills")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|name| {
            let source = Source::Git {
                url: url.clone(),
                rev: rev.clone(),
                subpath: String::new(),
            };
            (name.to_string(), source)
        })
        .collect()
}

/// `<claude>/plugins/installed_plugins.json`, keyed `<plugin>@<marketplace>`.
fn claude_plugins(claude: &Path) -> Vec<(PathBuf, Source)> {
    let cache = claude.join(CLAUDE_PLUGIN_CACHE);
    let Some(manifest) = json(&claude.join(CLAUDE_PLUGINS)) else {
        return Vec::new();
    };
    let Some(plugins) = manifest.get("plugins").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, installs) in plugins {
        let (plugin, marketplace) = key.split_once('@').unwrap_or((key, ""));
        for install in installs.as_array().into_iter().flatten() {
            let path = text(install, "installPath");
            let Some(path) = cache_relative(Path::new(&path), &cache) else {
                continue;
            };
            let source = Source::Plugin {
                marketplace: marketplace.to_string(),
                plugin: plugin.to_string(),
                version: text(install, "version"),
            };
            out.push((path, source));
        }
    }
    out
}

/// An `installPath` relative to the plugin cache. Stored absolute, so a config
/// root that was copied or moved keeps its old prefix: then the part after the
/// last `plugins/cache` pair of components is used.
fn cache_relative(install: &Path, cache: &Path) -> Option<PathBuf> {
    if let Ok(rest) = install.strip_prefix(cache) {
        return Some(rest.to_path_buf());
    }
    let parts: Vec<_> = install.components().collect();
    let at = parts.windows(2).rposition(
        |w| matches!(w, [a, b] if a.as_os_str() == "plugins" && b.as_os_str() == "cache"),
    )?;
    Some(parts.get(at + 2..)?.iter().collect())
}

/// The `[plugins."<key>"]` tables in `<codex>/config.toml` whose `enabled` is
/// true. A line scan, not a TOML parser: those tables are flat, and a parse
/// this narrow cannot misread the rest of the file into an answer.
fn codex_enabled(codex: &Path) -> BTreeSet<String> {
    let Ok(config) = fs::read_to_string(codex.join(CODEX_CONFIG)) else {
        return BTreeSet::new();
    };
    let mut enabled = BTreeSet::new();
    let mut current: Option<&str> = None;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            current = line
                .strip_prefix("[plugins.\"")
                .and_then(|rest| rest.strip_suffix("\"]"));
            continue;
        }
        let Some(key) = current else { continue };
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == "enabled" && v.split('#').next().is_some_and(|v| v.trim() == "true") {
                enabled.insert(key.to_string());
            }
        }
    }
    enabled
}

/// The plugin a Codex cache path belongs to, if Codex has it enabled. Cache
/// paths are `<marketplace>/<plugin>/<version>/skills/<name>` below the cache.
#[must_use]
pub fn codex_plugin(relative: &Path, enabled: &BTreeSet<String>) -> Option<Source> {
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let [marketplace, plugin, version, ..] = parts.as_slice() else {
        return None;
    };
    enabled
        .contains(&format!("{plugin}@{marketplace}"))
        .then(|| Source::Plugin {
            marketplace: marketplace.clone(),
            plugin: plugin.clone(),
            version: version.clone(),
        })
}

/// The installed Claude Code plugin whose directory holds a cache path.
#[must_use]
pub fn claude_plugin(relative: &Path, plugins: &[(PathBuf, Source)]) -> Option<Source> {
    plugins
        .iter()
        .find(|(dir, _)| relative.starts_with(dir))
        .map(|(_, source)| source.clone())
}
