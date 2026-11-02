//! The files that record where skills came from. Every reader is lenient: a
//! missing or malformed file means "nothing recorded", never an error, because
//! the doctor reports on a machine as it is.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::locations::Roots;
use crate::model::{SkillName, Source};

/// Relative to the home root.
pub const SKILL_LOCK: &str = ".agents/.skill-lock.json";
/// Relative to the home root.
pub const OPIK_SKILLS: &str = ".agents/skills/.opik-skills.json";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lockfiles {
    /// Sources for skills installed into `~/.agents/skills`, by name.
    pub tracked: BTreeMap<SkillName, Source>,
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
    Lockfiles { tracked }
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
