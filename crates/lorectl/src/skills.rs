//! `lorectl skills ls`: report on the skill folders every agent loads.
//! Read-only, and independent of the archive.

use std::io::Write;
use std::path::PathBuf;

use lore_skills::{Inventory, Roots, SkillRecord, Source};

use crate::cli::Invocation;
use crate::exit::{self, CliError};

/// The subcommands `skills` accepts.
pub const SUBCOMMANDS: &[&str] = &["ls"];

pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let subcommand = invocation.operand.as_deref().unwrap_or_default();
    if !SUBCOMMANDS.contains(&subcommand) {
        return Err(CliError::Usage(format!(
            "unknown `lorectl skills` subcommand `{subcommand}`; expected one of: {}",
            SUBCOMMANDS.join(", ")
        )));
    }
    let inventory = lore_skills::inventory(&roots()?);
    let out = if invocation.json {
        serde_json::to_string(&inventory.records).unwrap_or_default()
    } else {
        ls_text(&inventory)
    };
    let _ = writeln!(std::io::stdout().lock(), "{out}");
    Ok(exit::OK)
}

/// The home root to scan, from `HOME`. From the environment, so a test or a
/// live lane can point it at a fixture without a flag that would exist only
/// for them.
fn roots() -> Result<Roots, CliError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| CliError::Usage("HOME must be set to an absolute path".into()))?;
    Ok(Roots::from_home(&home))
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
