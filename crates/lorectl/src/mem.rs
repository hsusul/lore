//! `lorectl mem <SUBCOMMAND>`: memory notes in a git vault.
//!
//! The vault and its index live in `lore-memory`; this module parses the
//! subcommand's arguments and prints. It never opens the archive.

use std::io::{Read, Write};
use std::path::PathBuf;

use lexopt::prelude::*;
use lore_memory::anchor::{Freshness, WorkTree};
use lore_memory::note::Scope;
use lore_memory::recall::{self, Recall};
use lore_memory::repo::RepoContext;
use lore_memory::vault::{self, Added, Init, Vault};
use serde_json::json;

use crate::cli::Invocation;
use crate::exit::{self, CliError};

/// The `mem` section of `--help`, and all of `lorectl help mem`,
/// `lorectl mem --help`, and `lorectl mem <SUB> --help`.
pub const USAGE: &str = "\
Memory (lorectl mem <SUBCOMMAND> [--vault <DIR>]):
    init              Create the vault, a git repository Obsidian can open
    add <FILE>        Add a note file (`-` reads stdin); id, created, status,
                      and scope may be left out. The same file twice is one
                      note and one commit. An id another note has is refused.
                      --cwd <DIR> fills each anchor's missing hash and commit,
                      and the repository's name, from the repository at DIR
    show <NOTE>       Print a note by id, file name, or vault path
    search <QUERY>    Rank notes by BM25 [--limit N, default 10]
    recall [QUERY]    Global notes and the notes of the repository at --cwd
                      (default: here), each labeled fresh, moved, changed,
                      gone, or unanchored; all of them with no QUERY
                      [--limit N] [--json]
    reindex           Rebuild the index from the vault files alone, and name
                      any id that two files share (exit 7)
    propose           File an agent proposal; the body is read from stdin.
                      --agent, --session, --type, --title [--scope S]
                      [--anchor PATH[:LINES]]... [--updates|--extends|--derives ID]
    inbox             List pending proposals, each with its source session
                      and a redacted excerpt from the archive [--json]
    accept <ID>       Write a proposal as a note, one commit [--as <slug>]
    reject <ID>       Decline a proposal --reason <TEXT>; the vault is untouched
    retire <NOTE>     Mark a note retired --reason <TEXT>, one commit
    mine              Lessons from corrections and rules you gave agents,
                      most conversations first, each with its receipts;
                      writes nothing
                      [--limit N, default 20] [--json]. Reads the archive.
                      --propose ID... files those lessons into the inbox.
                      --min-overlap PERCENT (default 50): the share of
                      words two corrections need to be one lesson.
                      --max-chars N (default 300): longer turns are skipped
                      --all: also list lessons scored below zero, which
                      read as one-off orders
                      --new: only lessons with a correction no Find
                      lessons run has read yet
                      --replies: also list ordinary replies from the
                      last 48 hours that no rule flagged, one each
    eval              Score a policy for accepting proposals without you
                      against your own accepts and rejects; writes nothing
                      [--policy never|starting-bar, default never] [--json]
    enforce-check     Each active note's `enforce` rule, and how many of the
                      archive's tool calls from the last 30 days it would
                      have denied, with examples; writes nothing. Names
                      notes whose rule does not parse (exit 7)
                      [--cwd DIR: only rules in scope there]
    --vault <DIR>     Vault directory. Must be absolute
    Search and show pick up notes edited, renamed, or deleted by hand.
    Exit codes: 2 no vault, 5 vault busy, 7 note refused, 4 other failure.
";

const DEFAULT_LIMIT: usize = 10;

#[derive(Debug, PartialEq, Eq)]
enum Sub {
    Init,
    Add(String),
    Show(String),
    Search(String),
    Recall(String),
    Reindex,
}

#[derive(Debug, PartialEq, Eq)]
struct MemArgs {
    sub: Option<Sub>,
    vault: Option<PathBuf>,
    limit: usize,
    cwd: Option<PathBuf>,
    json: bool,
}

fn usage(message: impl Into<String>) -> CliError {
    CliError::Usage(message.into())
}

fn parse(
    sub: &str,
    rest: &[String],
    default_vault: Option<&std::path::Path>,
) -> Result<MemArgs, CliError> {
    let mut parser = lexopt::Parser::from_args(rest);
    let mut vault = default_vault.map(PathBuf::from);
    let mut limit = None;
    let mut cwd = None;
    let mut json = false;
    let mut help = false;
    let mut operand: Option<String> = None;
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Short('h') | Long("help") => help = true,
            Long("vault") => {
                vault = Some(PathBuf::from(
                    parser.value().map_err(|e| usage(e.to_string()))?,
                ));
            }
            Long("cwd") if sub == "recall" || sub == "add" => {
                cwd = Some(PathBuf::from(
                    parser.value().map_err(|e| usage(e.to_string()))?,
                ));
            }
            Long("json") if sub == "recall" => json = true,
            Long("limit") if sub == "search" || sub == "recall" => {
                let value = parser.value().map_err(|e| usage(e.to_string()))?;
                let value = value.to_string_lossy();
                limit = Some(
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or_else(|| {
                            usage(format!("--limit needs a positive number, got `{value}`"))
                        })?,
                );
            }
            Value(value) if operand.is_none() => {
                operand = Some(value.to_string_lossy().into_owned());
            }
            other => return Err(usage(other.unexpected().to_string())),
        }
    }
    if help {
        return Ok(MemArgs {
            sub: None,
            vault,
            limit: DEFAULT_LIMIT,
            cwd,
            json,
        });
    }
    let needs = |name: &str| {
        operand
            .clone()
            .ok_or_else(|| usage(format!("`lorectl mem {sub}` needs a {name}")))
    };
    let parsed = match sub {
        "init" | "reindex" if operand.is_some() => {
            return Err(usage(format!("`lorectl mem {sub}` takes no argument")))
        }
        "init" => Sub::Init,
        "reindex" => Sub::Reindex,
        "add" => Sub::Add(needs("FILE")?),
        "show" => Sub::Show(needs("NOTE")?),
        "search" => Sub::Search(needs("QUERY")?),
        "recall" => Sub::Recall(operand.clone().unwrap_or_default()),
        other => return Err(usage(format!("unknown mem subcommand `{other}`"))),
    };
    Ok(MemArgs {
        sub: Some(parsed),
        vault,
        limit: limit.unwrap_or(DEFAULT_LIMIT),
        cwd,
        json,
    })
}

/// Run `lorectl mem`.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let sub = invocation.operand.as_deref().unwrap_or_default();
    if sub == "mine" {
        return crate::mine::run(invocation, USAGE);
    }
    if sub == "eval" {
        return crate::eval::run(invocation, USAGE);
    }
    if sub == "enforce-check" {
        return crate::enforce_check::run(invocation, USAGE);
    }
    if crate::review::SUBCOMMANDS.contains(&sub) {
        let archive = invocation.archive_dir().ok();
        return crate::review::run(
            sub,
            &invocation.rest,
            invocation.json,
            USAGE,
            archive,
            invocation.vault.as_deref(),
        );
    }
    let args = parse(sub, &invocation.rest, invocation.vault.as_deref())?;
    let Some(sub) = args.sub else {
        print!("{USAGE}");
        return Ok(exit::OK);
    };
    let dir = vault::resolve_dir(args.vault.as_deref())?;
    let mut out = std::io::stdout().lock();
    match sub {
        Sub::Init => {
            let (vault, init) = Vault::init(&dir)?;
            let verb = match init {
                Init::Created => "initialized",
                Init::AlreadyInitialized => "already initialized",
            };
            let _ = writeln!(out, "{verb} vault at {}", vault.root().display());
        }
        Sub::Add(file) => {
            let text = read_input(&file)?;
            let vault = Vault::open(&dir)?;
            let (note, added) = match &args.cwd {
                None => vault.add(&text)?,
                Some(cwd) => {
                    let context = RepoContext::discover(cwd);
                    let root = context.root().ok_or_else(|| {
                        usage(format!(
                            "--cwd `{}` is not in a git repository",
                            cwd.display()
                        ))
                    })?;
                    let tree = WorkTree::new(root);
                    vault.add_with(&text, |note| {
                        for anchor in &mut note.anchors {
                            *anchor = tree.pin(anchor)?;
                        }
                        let ours =
                            matches!(&note.scope, Scope::Repo(key) if Some(key) == context.key());
                        if ours && note.repo.is_none() {
                            note.repo = context.name().cloned();
                        }
                        Ok(())
                    })?
                }
            };
            let verb = match added {
                Added::Committed => "added",
                Added::Unchanged => "unchanged",
            };
            let _ = writeln!(out, "{verb} {} {}", note.rel_path(), note.id.as_str());
        }
        Sub::Show(key) => {
            let vault = Vault::open(&dir)?;
            let text = vault.read(&vault.find(&key)?)?;
            let _ = out.write_all(text.as_bytes());
        }
        Sub::Search(query) => {
            let vault = Vault::open(&dir)?;
            let hits = vault.search(&query, args.limit)?;
            if hits.is_empty() {
                eprintln!("lorectl: no notes match `{query}`");
            }
            let width = hits
                .iter()
                .map(|h| h.slug.chars().count())
                .max()
                .unwrap_or(0);
            let scope_width = hits
                .iter()
                .map(|h| h.scope.chars().count())
                .max()
                .unwrap_or(0);
            for hit in hits {
                let _ = writeln!(
                    out,
                    "{:<width$}  {:<scope_width$}  {}",
                    hit.slug, hit.scope, hit.excerpt
                );
            }
        }
        Sub::Recall(query) => {
            let vault = Vault::open(&dir)?;
            let cwd = match args.cwd {
                Some(cwd) => cwd,
                None => std::env::current_dir()
                    .map_err(|e| usage(format!("cannot read the current directory: {e}")))?,
            };
            let found = recall::recall(&vault, &query, &cwd, args.limit)?;
            match &found.context {
                RepoContext::NoRepo => eprintln!(
                    "lorectl: {} is not in a git repository; recalling global notes only",
                    cwd.display()
                ),
                RepoContext::NoRemote { root } => eprintln!(
                    "lorectl: {} has no origin remote; recalling global notes only",
                    root.display()
                ),
                RepoContext::Repo { .. } => {}
            }
            for (rel, error) in &found.skipped {
                eprintln!("lorectl: skipped {rel}: {error}");
            }
            let json = args.json || invocation.json;
            if !json && found.notes.is_empty() {
                if lore_memory::index::has_words(&query) {
                    eprintln!("lorectl: no notes match `{query}`");
                } else {
                    eprintln!("lorectl: no notes in scope");
                }
            }
            if json {
                let _ = writeln!(out, "{}", recall_json(&found));
            } else {
                print_recall(&mut out, &found);
            }
        }
        Sub::Reindex => {
            let vault = Vault::open(&dir)?;
            let report = vault.reindex()?;
            for (rel, error) in &report.skipped {
                eprintln!("lorectl: skipped {rel}: {error}");
            }
            for duplicate in &report.duplicates {
                eprintln!(
                    "lorectl: id {} is in more than one note: {}",
                    duplicate.id.as_str(),
                    duplicate.paths.join(", ")
                );
            }
            let _ = writeln!(
                out,
                "indexed {} notes, skipped {} files",
                report.indexed,
                report.skipped.len()
            );
            if !report.duplicates.is_empty() {
                eprintln!("  give all but one of them a new id, then run `lorectl mem reindex`");
                return Ok(exit::NOTE_REFUSED);
            }
        }
    }
    Ok(exit::OK)
}

/// One line per note: the label, file name, scope, and excerpt, then where
/// each moved anchor is now.
fn print_recall(out: &mut impl Write, found: &Recall) {
    let slugs: Vec<&str> = found.notes.iter().map(|r| r.slug.as_str()).collect();
    let scopes: Vec<String> = found
        .notes
        .iter()
        .map(|r| r.note.scope.to_string())
        .collect();
    let width = slugs.iter().map(|s| s.chars().count()).max().unwrap_or(0);
    let scope_width = scopes.iter().map(|s| s.chars().count()).max().unwrap_or(0);
    for ((recalled, slug), scope) in found.notes.iter().zip(&slugs).zip(&scopes) {
        let _ = writeln!(
            out,
            "{:<10}  {slug:<width$}  {scope:<scope_width$}  {}",
            recalled.freshness.label(),
            recalled.excerpt
        );
        for located in &recalled.anchors {
            if let Freshness::Moved { lines } = located.freshness {
                let _ = writeln!(out, "{:<10}  -> {}:{lines}", "", located.path.as_str());
            }
        }
    }
}

fn recall_json(found: &Recall) -> serde_json::Value {
    let repo = match &found.context {
        RepoContext::NoRepo => json!({ "state": "no_repo" }),
        RepoContext::NoRemote { root } => {
            json!({ "state": "no_remote", "root": root.to_string_lossy() })
        }
        RepoContext::Repo { root, key, .. } => json!({
            "state": "repo",
            "root": root.to_string_lossy(),
            "scope": format!("repo:{}", key.as_str()),
        }),
    };
    let lines = |freshness: Freshness| match freshness {
        Freshness::Moved { lines } => Some(lines.to_string()),
        _ => None,
    };
    let notes: Vec<serde_json::Value> = found
        .notes
        .iter()
        .map(|r| {
            let anchors: Vec<serde_json::Value> = r
                .note
                .anchors
                .iter()
                .zip(&r.anchors)
                .map(|(anchor, located)| {
                    json!({
                        "path": anchor.path.as_str(),
                        "lines": anchor.lines.map(|l| l.to_string()),
                        "freshness": located.freshness.label(),
                        "now_path": located.path.as_str(),
                        "now_lines": lines(located.freshness),
                    })
                })
                .collect();
            json!({
                "id": r.note.id.as_str(),
                "path": r.path,
                "title": r.note.title,
                "type": r.note.note_type.as_str(),
                "scope": r.note.scope.to_string(),
                "freshness": r.freshness.label(),
                "anchors": anchors,
                "excerpt": r.excerpt,
                "score": r.score,
            })
        })
        .collect();
    let skipped: Vec<serde_json::Value> = found
        .skipped
        .iter()
        .map(|(rel, error)| json!({ "path": rel, "error": error.to_string() }))
        .collect();
    json!({ "repo": repo, "notes": notes, "skipped": skipped })
}

fn read_input(file: &str) -> Result<String, CliError> {
    let mut text = String::new();
    let read = if file == "-" {
        std::io::stdin().read_to_string(&mut text).map(|_| ())
    } else {
        std::fs::read_to_string(file).map(|t| text = t)
    };
    read.map_err(|e| usage(format!("cannot read `{file}`: {e}")))?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(sub: &str, rest: &[&str]) -> Result<MemArgs, CliError> {
        let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
        parse(sub, &rest, None)
    }

    #[test]
    fn default_vault_is_used_when_not_overridden() {
        let default_vault = std::path::Path::new("/default/日本語_vault");
        let parsed = parse("init", &[], Some(default_vault)).unwrap();
        assert_eq!(parsed.vault, Some(PathBuf::from("/default/日本語_vault")));

        let rest_override: Vec<String> = vec!["--vault".into(), "/override/日本語_vault".into()];
        let parsed_override = parse("init", &rest_override, Some(default_vault)).unwrap();
        assert_eq!(
            parsed_override.vault,
            Some(PathBuf::from("/override/日本語_vault"))
        );
    }

    #[test]
    fn every_subcommand_parses() {
        assert_eq!(args("init", &[]).unwrap().sub, Some(Sub::Init));
        assert_eq!(args("reindex", &[]).unwrap().sub, Some(Sub::Reindex));
        assert_eq!(
            args("add", &["n.md"]).unwrap().sub,
            Some(Sub::Add("n.md".into()))
        );
        assert_eq!(
            args("show", &["x"]).unwrap().sub,
            Some(Sub::Show("x".into()))
        );
        let search = args("search", &["two words", "--limit", "3", "--vault", "/v"]).unwrap();
        assert_eq!(search.sub, Some(Sub::Search("two words".into())));
        assert_eq!((search.limit, search.vault), (3, Some(PathBuf::from("/v"))));
        let recall = args("recall", &["q", "--cwd", "/r", "--json", "--limit", "4"]).unwrap();
        assert_eq!(recall.sub, Some(Sub::Recall("q".into())));
        assert_eq!(
            (recall.cwd, recall.json, recall.limit),
            (Some(PathBuf::from("/r")), true, 4)
        );
        let everything = args("recall", &["--cwd", "/r"]).unwrap();
        assert_eq!(everything.sub, Some(Sub::Recall(String::new())));
        let add = args("add", &["n.md", "--cwd", "/r"]).unwrap();
        assert_eq!(add.cwd, Some(PathBuf::from("/r")));
    }

    #[test]
    fn bad_arguments_are_usage_errors() {
        for (sub, rest) in [
            ("frob", &[][..]),
            ("add", &[][..]),
            ("init", &["x"][..]),
            ("search", &["q", "extra"][..]),
            ("search", &["q", "--limit", "0"][..]),
            ("show", &["x", "--limit", "3"][..]),
            ("add", &["x", "--wat"][..]),
            ("recall", &["q", "extra"][..]),
            ("search", &["q", "--json"][..]),
            ("show", &["x", "--cwd", "/r"][..]),
            ("add", &["x", "--json"][..]),
        ] {
            let error = args(sub, rest).unwrap_err();
            assert_eq!(exit::exit_code(&error), exit::USAGE, "{sub} {rest:?}");
        }
    }

    #[test]
    fn help_wins_over_a_missing_operand() {
        assert_eq!(args("add", &["--help"]).unwrap().sub, None);
    }
}
