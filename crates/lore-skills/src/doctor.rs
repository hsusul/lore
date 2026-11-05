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
    let reach = if inventory.records.iter().any(|r| !r.refs.is_empty()) {
        reachable(&inventory.records)
    } else {
        BTreeMap::new()
    };

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
    let total_installs: usize = records.iter().map(|r| r.installs.len()).sum();
    if total_installs <= 1 {
        return None;
    }
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
    let mut unique_copies: Vec<(usize, &Path, &Path)> = Vec::with_capacity(copies.len());
    for copy in copies {
        if !unique_copies.iter().any(|c| c.1 == copy.1) {
            unique_copies.push(copy);
        }
    }
    let has_multiple_locations = unique_copies
        .first()
        .is_some_and(|first| unique_copies.last().is_some_and(|last| first.0 != last.0));
    has_multiple_locations.then(|| Finding::Duplicate {
        skill: name.to_string(),
        paths: unique_copies
            .into_iter()
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
    let get_owned = || {
        record
            .installs
            .iter()
            .find(|i| !managed(i, roots))
            .map(|i| i.path.clone())
    };
    if let Some(reason) = &record.frontmatter_error {
        let path = first_path(record);
        out.push(Finding::BadFrontmatter {
            skill: record.name.clone(),
            path: get_owned().unwrap_or(path),
            reason: reason.clone(),
        });
    } else if let Some(declared) = record.declared_name.as_ref().filter(|d| **d != record.name) {
        // A vendor that names a skill apart from its folder in a copy its
        // agent manages (Hugging Face's `cli` declares `hf-cli`) chose that
        // name, and the agent loads it as declared. Only a copy the user owns
        // is theirs to rename.
        if let Some(owned) = get_owned() {
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
            path: first_path(record),
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
    if record.refs.is_empty() {
        return Vec::new();
    }
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
    let mut worst: BTreeMap<String, u8> = BTreeMap::new();
    let mut none_worst = u8::MAX;
    for finding in findings.iter() {
        if let Some(skill) = finding.skill() {
            if let Some(rank) = worst.get_mut(skill) {
                *rank = (*rank).min(finding.rank());
            } else {
                worst.insert(skill.to_string(), finding.rank());
            }
        } else {
            none_worst = none_worst.min(finding.rank());
        }
    }
    findings.sort_by(|a, b| {
        let a_group = a
            .skill()
            .and_then(|s| worst.get(s).copied())
            .unwrap_or(none_worst);
        let b_group = b
            .skill()
            .and_then(|s| worst.get(s).copied())
            .unwrap_or(none_worst);
        (a_group, a.skill(), a.rank()).cmp(&(b_group, b.skill(), b.rank()))
    });
}
