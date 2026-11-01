//! Turn an [`Inventory`] into [`Finding`]s, grouped by skill, worst first.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::locations::{self, Roots};
use crate::model::{Agent, DriftCopy, Finding, Install, SkillRecord, Source};
use crate::scan::Inventory;

/// Codex's budget for the skill list it puts in context: names plus
/// descriptions, in characters.
pub const CODEX_BUDGET_CHARS: usize = 8_000;

#[must_use]
pub fn doctor(inventory: &Inventory) -> Vec<Finding> {
    let mut by_name: BTreeMap<&str, Vec<&SkillRecord>> = BTreeMap::new();
    for record in &inventory.records {
        by_name.entry(&record.name).or_default().push(record);
    }
    let reach = reachable(&inventory.records);

    let mut findings = Vec::new();
    for (name, records) in &by_name {
        if let Some(finding) = duplicate(name, records) {
            findings.push(finding);
        }
        if records.len() > 1 {
            findings.push(Finding::Drift {
                skill: (*name).to_string(),
                copies: records
                    .iter()
                    .map(|r| DriftCopy {
                        path: first_path(r),
                        hash: r.hash.clone(),
                        lines: r.lines,
                    })
                    .collect(),
            });
        }
        for record in records {
            findings.extend(per_record(record, &inventory.roots));
            findings.extend(broken_refs(record, &reach, &by_name, inventory));
        }
    }
    findings.extend(codex_budget(&by_name));
    sort(&mut findings);
    findings
}

fn first_path(record: &SkillRecord) -> PathBuf {
    record
        .installs
        .first()
        .map(|i| i.path.clone())
        .unwrap_or_default()
}

/// Separate copies on disk in two or more locations. Links to one folder are
/// one copy, so they never count.
fn duplicate(name: &str, records: &[&SkillRecord]) -> Option<Finding> {
    let mut copies: Vec<(usize, &Path, &Path)> = records
        .iter()
        .flat_map(|r| &r.installs)
        .map(|i| {
            (
                locations::order(i.location),
                i.real_path.as_path(),
                i.path.as_path(),
            )
        })
        .collect();
    copies.sort();
    let mut seen_real = BTreeSet::new();
    copies.retain(|(_, real, _)| seen_real.insert(*real));
    let locations: BTreeSet<usize> = copies.iter().map(|(order, ..)| *order).collect();
    (locations.len() > 1).then(|| Finding::Duplicate {
        skill: name.to_string(),
        paths: copies
            .iter()
            .map(|(_, _, path)| path.to_path_buf())
            .collect(),
    })
}

/// Whether an agent writes this copy: it sits in a managed location, or links
/// into one.
fn managed(install: &Install, roots: &Roots) -> bool {
    locations::LOCATIONS
        .iter()
        .any(|l| l.id == install.location && l.managed_by.is_some())
        || roots.manager(&install.real_path).is_some()
}

fn per_record(record: &SkillRecord, roots: &Roots) -> Vec<Finding> {
    let mut out = Vec::new();
    let path = first_path(record);
    // The copy a fix may point at: the first one the user owns.
    let owned = record
        .installs
        .iter()
        .find(|i| !managed(i, roots))
        .map(|i| i.path.clone());
    if let Some(reason) = &record.frontmatter_error {
        out.push(Finding::BadFrontmatter {
            skill: record.name.clone(),
            path: owned.unwrap_or_else(|| path.clone()),
            reason: reason.clone(),
        });
    } else if let Some(declared) = record.declared_name.as_ref().filter(|d| **d != record.name) {
        // A vendor that names a skill apart from its folder in a copy its
        // agent manages (Hugging Face's `cli` declares `hf-cli`) chose that
        // name, and the agent loads it as declared. Only a copy the user owns
        // is theirs to rename.
        if let Some(owned) = owned {
            out.push(Finding::NameMismatch {
                skill: record.name.clone(),
                declared: declared.clone(),
                path: owned,
            });
        }
    }
    if record.source == Source::Unknown {
        out.push(Finding::UnknownSource {
            skill: record.name.clone(),
            path,
        });
    }
    out
}

/// Every skill name each agent can load.
fn reachable(records: &[SkillRecord]) -> BTreeMap<Agent, BTreeSet<&str>> {
    let mut reach: BTreeMap<Agent, BTreeSet<&str>> = BTreeMap::new();
    for record in records {
        for agent in record.installs.iter().flat_map(|i| &i.agents) {
            reach.entry(*agent).or_default().insert(&record.name);
        }
    }
    reach
}

fn broken_refs(
    record: &SkillRecord,
    reach: &BTreeMap<Agent, BTreeSet<&str>>,
    by_name: &BTreeMap<&str, Vec<&SkillRecord>>,
    inventory: &Inventory,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut agents: BTreeMap<Agent, &Path> = BTreeMap::new();
    for install in &record.installs {
        for agent in &install.agents {
            agents.entry(*agent).or_insert(&install.path);
        }
    }
    for missing in &record.refs {
        let found_at = by_name
            .get(missing.as_str())
            .and_then(|rs| rs.first())
            .map(|r| first_path(r))
            .or_else(|| inventory.elsewhere.get(missing).cloned());
        let Some(found_at) = found_at else { continue };
        for (agent, path) in &agents {
            let loads = reach
                .get(agent)
                .is_some_and(|names| names.contains(missing.as_str()));
            if !loads {
                out.push(Finding::BrokenRef {
                    skill: record.name.clone(),
                    missing: missing.clone(),
                    agent: *agent,
                    path: path.to_path_buf(),
                    found_at: found_at.clone(),
                });
            }
        }
    }
    out
}

/// One entry per name Codex loads, measured from the copy it finds first.
fn codex_budget(by_name: &BTreeMap<&str, Vec<&SkillRecord>>) -> Option<Finding> {
    let mut chars = 0;
    let mut skills = 0;
    let mut largest: Option<(usize, PathBuf)> = None;
    for records in by_name.values() {
        let loaded = records
            .iter()
            .find(|r| r.installs.iter().any(|i| i.agents.contains(&Agent::Codex)));
        let Some(record) = loaded else { continue };
        let name = record.declared_name.as_deref().unwrap_or(&record.name);
        let size = name.chars().count()
            + record
                .description
                .as_deref()
                .map_or(0, |d| d.chars().count());
        chars += size;
        skills += 1;
        if largest.as_ref().is_none_or(|(most, _)| size > *most) {
            largest = Some((size, first_path(record)));
        }
    }
    let (_, largest) = largest?;
    (chars > CODEX_BUDGET_CHARS).then_some(Finding::CodexBudget {
        chars,
        limit: CODEX_BUDGET_CHARS,
        skills,
        largest,
    })
}

/// Group by skill; groups ordered by their worst finding, then by name; the
/// agent-wide `CodexBudget` sorts as its own group.
fn sort(findings: &mut [Finding]) {
    let mut worst: BTreeMap<Option<String>, u8> = BTreeMap::new();
    for finding in findings.iter() {
        let key = finding.skill().map(str::to_string);
        let rank = worst.entry(key).or_insert(u8::MAX);
        *rank = (*rank).min(finding.rank());
    }
    findings.sort_by_cached_key(|f| {
        let key = f.skill().map(str::to_string);
        let group = worst.get(&key).copied().unwrap_or(u8::MAX);
        (group, key, f.rank())
    });
}
