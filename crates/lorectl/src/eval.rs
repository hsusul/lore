//! `lorectl mem eval`: score a policy for accepting proposals without a person
//! against the proposals the person already accepted and rejected.
//!
//! It reads the vault's inbox and decision log, and the archive to tell which
//! proposal sessions it holds. It writes nothing and accepts nothing.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use lexopt::prelude::*;
use lore_core::paths::{self, ARCHIVE_DB_FILENAME};
use lore_core::{query, storage};
use lore_memory::eval::{self, Case, Gate, Policy, Report};
use lore_memory::inbox;
use lore_memory::policy::StartingBar;
use lore_memory::vault::{self, Vault};
use serde_json::json;

use crate::cli::Invocation;
use crate::exit::{self, CliError};

const POLICIES: &str = "never, starting-bar";

/// Run `lorectl mem eval`.
pub fn run(invocation: &Invocation, usage_text: &str) -> Result<u8, CliError> {
    let usage = |m: String| CliError::Usage(m);
    let mut parser = lexopt::Parser::from_args(&invocation.rest);
    let (mut policy, mut vault_dir, mut json) = (
        String::from("never"),
        invocation.vault.clone(),
        invocation.json,
    );
    let mut archive_dir = invocation.archive.clone();
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Short('h') | Long("help") => {
                print!("{usage_text}");
                return Ok(exit::OK);
            }
            Long("policy") => {
                let value = parser.value().map_err(|e| usage(e.to_string()))?;
                policy = value.to_string_lossy().into_owned();
            }
            Long("vault") => {
                let value = parser.value().map_err(|e| usage(e.to_string()))?;
                vault_dir = Some(PathBuf::from(value));
            }
            Long("archive") => {
                let value = parser.value().map_err(|e| usage(e.to_string()))?;
                archive_dir = Some(PathBuf::from(value));
            }
            Long("json") => json = true,
            other => return Err(usage(other.unexpected().to_string())),
        }
    }
    let vault = Vault::open(&vault::resolve_dir(vault_dir.as_deref())?)?;
    let cases = eval::cases(&vault)?;
    let policy: Box<dyn Policy> = match policy.as_str() {
        "never" => Box::new(eval::Never),
        "starting-bar" => Box::new(StartingBar::new(verified(archive_dir.as_deref(), &cases))),
        other => return Err(usage(format!("unknown policy `{other}`; use {POLICIES}"))),
    };
    let report = eval::score(policy.as_ref(), &cases);
    let shortfalls = report.shortfalls(&Gate::default());
    let mut out = std::io::stdout().lock();
    let text = if json {
        format!("{}\n", report_json(policy.name(), &report, &shortfalls))
    } else {
        report_text(policy.name(), &report, &shortfalls)
    };
    let _ = out.write_all(text.as_bytes());
    Ok(exit::OK)
}

/// The proposal sessions the archive holds for the agent that proposed under
/// them. None when there is no archive.
fn verified(archive: Option<&Path>, cases: &[Case]) -> BTreeSet<String> {
    let conn = paths::archive_dir(archive)
        .ok()
        .and_then(|dir| storage::open_read_only(&dir.join(ARCHIVE_DB_FILENAME)).ok());
    let Some(conn) = conn else {
        return BTreeSet::new();
    };
    let held = |case: &&Case| {
        let session = case.proposal.session.as_str();
        query::find_session(&conn, session)
            .ok()
            .flatten()
            .is_some_and(|o| o.agent_id == case.proposal.agent.as_str())
    };
    (cases.iter().filter(held))
        .map(|case| case.proposal.session.as_str().to_owned())
        .collect()
}

fn per_mille(part: usize, whole: usize) -> Option<usize> {
    (part * 1000).checked_div(whole)
}

fn report_text(policy: &str, report: &Report, shortfalls: &[String]) -> String {
    let mut out = format!(
        "Policy {policy}, scored on {} proposals you decided: {} accepted, {} rejected.\n\
         It would accept {}: {} you accepted and {} you rejected.\n",
        report.cases,
        report.accepts,
        report.rejects,
        report.auto_accepted,
        report.agreed,
        report.false_accepts.len(),
    );
    let precision = per_mille(report.agreed, report.auto_accepted)
        .map_or_else(|| "none accepted".to_owned(), |p| format!("{p}/1000"));
    let coverage = per_mille(report.agreed, report.accepts)
        .map_or_else(|| "none to cover".to_owned(), |c| format!("{c}/1000"));
    out.push_str(&format!("Precision: {precision}. Coverage: {coverage}.\n"));
    for proposal in &report.false_accepts {
        out.push_str(&format!(
            "- would accept {}, which you rejected: {}\n",
            proposal.id.as_str(),
            title(&proposal.title)
        ));
    }
    if shortfalls.is_empty() {
        out.push_str("Gate passed.\n");
    } else {
        out.push_str("Gate not passed:\n");
        for shortfall in shortfalls {
            out.push_str(&format!("- {shortfall}\n"));
        }
    }
    out
}

fn report_json(policy: &str, report: &Report, shortfalls: &[String]) -> serde_json::Value {
    let false_accepts: Vec<_> = (report.false_accepts.iter())
        .map(|p| json!({ "id": p.id.as_str(), "title": title(&p.title) }))
        .collect();
    json!({
        "policy": policy,
        "cases": report.cases,
        "accepts": report.accepts,
        "rejects": report.rejects,
        "auto_accepted": report.auto_accepted,
        "agreed": report.agreed,
        "false_accepts": false_accepts,
        "precision_per_mille": per_mille(report.agreed, report.auto_accepted),
        "coverage_per_mille": per_mille(report.agreed, report.accepts),
        "passes": shortfalls.is_empty(),
        "shortfalls": shortfalls,
    })
}

/// A proposal title as stored is already redacted; mask again in case the
/// scanner's rules grew since.
fn title(text: &str) -> String {
    inbox::redact(text).unwrap_or_else(|_| "(title withheld)".to_owned())
}
