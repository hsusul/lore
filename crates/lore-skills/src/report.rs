//! The doctor's findings as a person reads them: each with a one-line
//! description, paths shown under `~`, and its fix, plus a one-line summary.
//! `lorectl skills doctor` prints this, and the desktop app's Skills doctor
//! shows the same entries.

use std::path::Path;

use crate::{fixes, Finding, Fix, Inventory};

/// One finding, described, with the fix for it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub finding: Finding,
    pub description: String,
    pub fix: Fix,
}

/// The doctor's findings over `inventory`, in its order, each described.
#[must_use]
pub fn report(inventory: &Inventory, findings: &[Finding]) -> Vec<Entry> {
    let show = |path: &Path| match path.strip_prefix(&inventory.roots.home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    };
    findings
        .iter()
        .zip(fixes(findings, &inventory.roots))
        .map(|(finding, fix)| Entry {
            finding: finding.clone(),
            description: describe(finding, &show),
            fix,
        })
        .collect()
}

/// How many differently named skills `inventory` holds.
#[must_use]
pub fn distinct_names(inventory: &Inventory) -> usize {
    let mut names: Vec<&str> = inventory.records.iter().map(|r| r.name.as_str()).collect();
    names.dedup();
    names.len()
}

fn describe(finding: &Finding, show: &dyn Fn(&Path) -> String) -> String {
    match finding {
        Finding::BadFrontmatter { path, reason, .. } => {
            format!("{}/SKILL.md {reason}", show(path))
        }
        Finding::NameMismatch { declared, path, .. } => {
            format!("{} declares `name: {declared}`", show(path))
        }
        Finding::BrokenRef {
            missing,
            agent,
            path,
            found_at,
            ..
        } => format!(
            "{} names `{missing}`, which {agent} cannot load (it is at {})",
            show(path),
            show(found_at)
        ),
        Finding::Drift { copies, .. } => {
            let copies: Vec<String> = copies
                .iter()
                .map(|c| format!("{} ({} lines, {})", show(&c.path), c.lines, c.hash.short()))
                .collect();
            format!("{} copies differ: {}", copies.len(), copies.join(", "))
        }
        Finding::CodexBudget {
            chars,
            limit,
            skills,
            ..
        } => format!("{skills} skills Codex loads use {chars} characters of names and descriptions, over its {limit}"),
        Finding::Duplicate { paths, .. } => {
            let paths: Vec<String> = paths.iter().map(|p| show(p)).collect();
            format!("{} separate copies: {}", paths.len(), paths.join(", "))
        }
        Finding::UnknownSource { path, .. } => {
            format!("{} is in no lockfile or plugin manifest", show(path))
        }
    }
}

/// One line, duplicates first.
#[must_use]
pub fn summary(findings: &[Finding], skills: usize) -> String {
    let count = |kind: &str| findings.iter().filter(|f| f.kind() == kind).count();
    format!(
        "{} duplicates, {} drift, {} unknown sources, {} broken refs, {} name mismatches, \
         {} bad frontmatter, {} codex budget; {} findings; {skills} skills found",
        count("duplicate"),
        count("drift"),
        count("unknown-source"),
        count("broken-ref"),
        count("name-mismatch"),
        count("bad-frontmatter"),
        count("codex-budget"),
        findings.len(),
    )
}
