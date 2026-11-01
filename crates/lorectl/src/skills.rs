//! `lorectl skills ls` and `lorectl skills doctor`: report on the skill
//! folders every agent loads. Read-only, and independent of the archive.
//!
//! `doctor` exits 0 whenever the scan ran, findings or not: it reports, and a
//! hook or script that wants to gate on findings reads `--json`.

use std::io::Write;
use std::path::{Path, PathBuf};

use lore_skills::{Finding, Inventory, Roots, SkillRecord, Source};

use crate::cli::Invocation;
use crate::exit::{self, CliError};

/// The subcommands `skills` accepts.
pub const SUBCOMMANDS: &[&str] = &["ls", "doctor"];

pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let subcommand = invocation.operand.as_deref().unwrap_or_default();
    if !SUBCOMMANDS.contains(&subcommand) {
        return Err(CliError::Usage(format!(
            "unknown `lorectl skills` subcommand `{subcommand}`; expected one of: {}",
            SUBCOMMANDS.join(", ")
        )));
    }
    let inventory = lore_skills::inventory(&roots()?);
    let out = match (subcommand, invocation.json) {
        ("ls", true) => serde_json::to_string(&inventory.records).unwrap_or_default(),
        ("ls", false) => ls_text(&inventory),
        (_, true) => doctor_json(&inventory, &lore_skills::doctor(&inventory)),
        (_, false) => doctor_text(&inventory, &lore_skills::doctor(&inventory)),
    };
    let _ = writeln!(std::io::stdout().lock(), "{out}");
    Ok(exit::OK)
}

/// The roots to scan: `HOME`, with Claude Code's and Codex's folders moved by
/// `CLAUDE_CONFIG_DIR` and `CODEX_HOME` the way the session adapters move
/// them. From the environment, so a test or a live lane can point them at a
/// fixture without a flag that would exist only for them.
fn roots() -> Result<Roots, CliError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| CliError::Usage("HOME must be set to an absolute path".into()))?;
    Ok(Roots::resolve(
        &home,
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        std::env::var_os("CODEX_HOME").map(PathBuf::from),
    ))
}

fn distinct_names(inventory: &Inventory) -> usize {
    let mut names: Vec<&str> = inventory.records.iter().map(|r| r.name.as_str()).collect();
    names.dedup();
    names.len()
}

fn ls_text(inventory: &Inventory) -> String {
    let width = inventory
        .records
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for record in &inventory.records {
        let locations: Vec<&str> = record.installs.iter().map(|i| i.location).collect();
        out.push_str(&format!(
            "{:<width$}  {}  {:<40}  {}\n",
            record.name,
            record.hash.short(),
            source_label(record),
            locations.join(",")
        ));
    }
    out.push_str(&format!("{} skills found", distinct_names(inventory)));
    out
}

fn source_label(record: &SkillRecord) -> String {
    match &record.source {
        Source::Git { url, .. } => format!("git {url}"),
        Source::Local { path } => format!("local {}", path.display()),
        Source::Plugin {
            marketplace,
            plugin,
            version,
        } => format!("plugin {plugin}@{marketplace} {version}"),
        Source::Builtin { agent } => format!("builtin {agent}"),
        Source::Unknown => "unknown".to_string(),
    }
}

/// The findings as one JSON array, each with its `fix` and whether that fix
/// is a `command` or a `hint`.
fn doctor_json(inventory: &Inventory, findings: &[Finding]) -> String {
    let records: Vec<serde_json::Value> = findings
        .iter()
        .map(|f| {
            let mut value = serde_json::to_value(f).unwrap_or_default();
            if let Some(object) = value.as_object_mut() {
                let fix = f.fix(&inventory.roots);
                object.insert("fix".into(), fix.text().into());
                object.insert("fix_kind".into(), fix.kind().into());
            }
            value
        })
        .collect();
    serde_json::Value::Array(records).to_string()
}

fn doctor_text(inventory: &Inventory, findings: &[Finding]) -> String {
    let show = |path: &Path| match path.strip_prefix(&inventory.roots.home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    };
    let mut out = String::new();
    let mut group: Option<Option<&str>> = None;
    for finding in findings {
        if group != Some(finding.skill()) {
            group = Some(finding.skill());
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(finding.skill().unwrap_or("(codex skill list)"));
            out.push('\n');
        }
        let fix = finding.fix(&inventory.roots);
        let label = match fix.kind() {
            "command" => "fix",
            other => other,
        };
        out.push_str(&format!(
            "  {:<15} {}\n  {:<15} {}\n",
            finding.kind(),
            describe(finding, &show),
            label,
            fix.text()
        ));
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&summary(findings, distinct_names(inventory)));
    out
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
fn summary(findings: &[Finding], skills: usize) -> String {
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
