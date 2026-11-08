//! The files that record where skills came from. Every reader is lenient: a
//! missing or malformed file means "nothing recorded", never an error, because
//! the doctor reports on a machine as it is.

use std::cmp::Ordering;
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
/// Relative to Claude Code's config root.
pub const CLAUDE_SETTINGS: &str = "settings.json";
/// Relative to Codex's home.
pub const CODEX_CONFIG: &str = "config.toml";
/// Relative to Codex's home.
pub const CODEX_PLUGIN_CACHE: &str = "plugins/cache";
/// In `<cache>/<marketplace>/<plugin>/`: Codex writes it when it installs a
/// plugin from a remote catalog. Those installs are enabled on the account,
/// not in `config.toml`.
pub const CODEX_REMOTE_INSTALL: &str = ".codex-remote-plugin-install.json";
/// The cached version Codex prefers over any other when present.
const CODEX_LOCAL_VERSION: &str = "local";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lockfiles {
    /// Sources for skills installed into `~/.agents/skills`, by name.
    pub tracked: BTreeMap<SkillName, Source>,
    /// Claude Code plugin install directories, relative to the plugin cache,
    /// and the plugin each holds. Plugins `settings.json` disables are left
    /// out.
    pub claude_plugins: Vec<(PathBuf, Source)>,
    /// `<plugin>@<marketplace>` keys Codex loads, each with the cached version
    /// it loads: enabled in `config.toml`, or installed from a remote catalog
    /// and not disabled there.
    pub codex_enabled: BTreeMap<String, String>,
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
        codex_enabled: codex_loaded(&roots.codex),
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
            let source = if text(entry, "sourceType") == "local" {
                Source::Local {
                    path: PathBuf::from(text(entry, "source")),
                }
            } else {
                let url = match text(entry, "sourceUrl") {
                    url if url.is_empty() => text(entry, "source"),
                    url => url,
                };
                Source::Git {
                    url,
                    rev: text(entry, "skillFolderHash"),
                    subpath: text(entry, "skillPath"),
                }
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

/// `<claude>/plugins/installed_plugins.json`, keyed `<plugin>@<marketplace>`,
/// less any plugin `<claude>/settings.json` sets to `false` in
/// `enabledPlugins`.
fn claude_plugins(claude: &Path) -> Vec<(PathBuf, Source)> {
    let cache = claude.join(CLAUDE_PLUGIN_CACHE);
    let Some(manifest) = json(&claude.join(CLAUDE_PLUGINS)) else {
        return Vec::new();
    };
    let Some(plugins) = manifest.get("plugins").and_then(Value::as_object) else {
        return Vec::new();
    };
    let settings = json(&claude.join(CLAUDE_SETTINGS));
    let enabled = settings
        .as_ref()
        .and_then(|s| s.get("enabledPlugins"))
        .and_then(Value::as_object);
    let mut out = Vec::new();
    for (key, installs) in plugins {
        let disabled = enabled
            .and_then(|e| e.get(key))
            .and_then(Value::as_bool)
            .is_some_and(|on| !on);
        if disabled {
            continue;
        }
        let (plugin, marketplace) = key.rsplit_once('@').unwrap_or((key, ""));
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

/// The plugins Codex loads, each at its active cached version. A plugin's
/// `enabled` in `config.toml` decides; with none, a remote install marker in
/// the cache means enabled, since the remote catalog only caches what the
/// account has installed.
fn codex_loaded(codex: &Path) -> BTreeMap<String, String> {
    let config = codex_config_enabled(codex);
    let cache = codex.join(CODEX_PLUGIN_CACHE);
    let mut keys: BTreeSet<String> = config
        .iter()
        .filter(|(_, on)| **on)
        .map(|(key, _)| key.clone())
        .collect();
    for (marketplace, plugin) in codex_remote_installs(&cache) {
        let key = format!("{plugin}@{marketplace}");
        if !config.contains_key(&key) {
            keys.insert(key);
        }
    }
    keys.into_iter()
        .filter_map(|key| {
            let (plugin, marketplace) = key.rsplit_once('@')?;
            let version = codex_active_version(&cache.join(marketplace).join(plugin))?;
            Some((key, version))
        })
        .collect()
}

/// `(marketplace, plugin)` for every cached plugin with a remote install
/// marker.
fn codex_remote_installs(cache: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for marketplace in dir_names(cache) {
        for plugin in dir_names(&cache.join(&marketplace)) {
            let marker = cache
                .join(&marketplace)
                .join(&plugin)
                .join(CODEX_REMOTE_INSTALL);
            if marker.is_file() {
                out.push((marketplace.clone(), plugin));
            }
        }
    }
    out
}

/// Names of the real directories in `dir`; links are not followed.
fn dir_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// The version folder Codex loads a plugin from, as Codex picks it: a real
/// directory (a `latest` link does not count) with a valid version name,
/// `local` if present, else the highest by semver, else by text.
#[must_use]
pub fn codex_active_version(plugin_dir: &Path) -> Option<String> {
    let versions: Vec<String> = dir_names(plugin_dir)
        .into_iter()
        .filter(|v| {
            !matches!(v.as_str(), "." | "..")
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
        })
        .collect();
    if versions.iter().any(|v| v == CODEX_LOCAL_VERSION) {
        return Some(CODEX_LOCAL_VERSION.to_string());
    }
    versions.into_iter().max_by(|a, b| compare_versions(a, b))
}

/// Semver precedence when both parse, else text order, as Codex compares.
fn compare_versions(a: &str, b: &str) -> Ordering {
    match (Semver::parse(a), Semver::parse(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

/// `MAJOR.MINOR.PATCH[-pre][+build]`, ordered by semver precedence. Build
/// metadata is kept last as a tie-break, as the `semver` crate does.
#[derive(PartialEq, Eq)]
struct Semver<'a> {
    core: [u64; 3],
    pre: Vec<&'a str>,
    build: &'a str,
}

impl<'a> Semver<'a> {
    fn parse(text: &'a str) -> Option<Self> {
        let (rest, build) = text.split_once('+').unwrap_or((text, ""));
        let (core, pre) = rest
            .split_once('-')
            .map_or((rest, None), |(c, p)| (c, Some(p)));
        let numbers: Vec<u64> = core
            .split('.')
            .map(|n| {
                let leading_zero = n.len() > 1 && n.starts_with('0');
                (!leading_zero).then(|| n.parse().ok()).flatten()
            })
            .collect::<Option<_>>()?;
        let core: [u64; 3] = numbers.try_into().ok()?;
        let pre: Vec<&str> = match pre {
            Some(p) => p.split('.').collect(),
            None => Vec::new(),
        };
        let ident =
            |s: &&str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !pre.iter().all(ident) || (!build.is_empty() && !build.split('.').all(|s| ident(&s))) {
            return None;
        }
        Some(Self { core, pre, build })
    }
}

impl Ord for Semver<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&other.pre) {
                        let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                            (Ok(a), Ok(b)) => a.cmp(&b),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => a.cmp(b),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            })
            .then_with(|| self.build.cmp(other.build))
    }
}

impl PartialOrd for Semver<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The `[plugins."<key>"]` tables in `<codex>/config.toml` and whether each
/// sets `enabled = true`. A line scan, not a TOML parser: those tables are
/// flat, and a parse this narrow cannot misread the rest of the file into an
/// answer.
fn codex_config_enabled(codex: &Path) -> BTreeMap<String, bool> {
    let Ok(config) = fs::read_to_string(codex.join(CODEX_CONFIG)) else {
        return BTreeMap::new();
    };
    let mut enabled = BTreeMap::new();
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
            if k.trim() == "enabled" {
                let on = v.split('#').next().is_some_and(|v| v.trim() == "true");
                enabled.insert(key.to_string(), on);
            }
        }
    }
    enabled
}

/// The plugin a Codex cache path belongs to, if Codex loads that plugin and
/// the path is in its active version. Cache paths are
/// `<marketplace>/<plugin>/<version>/skills/<name>` below the cache.
#[must_use]
pub fn codex_plugin(relative: &Path, enabled: &BTreeMap<String, String>) -> Option<Source> {
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let [marketplace, plugin, version, ..] = parts.as_slice() else {
        return None;
    };
    enabled
        .get(&format!("{plugin}@{marketplace}"))
        .is_some_and(|active| active == version)
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
