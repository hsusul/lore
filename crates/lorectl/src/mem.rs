//! `lorectl mem <SUBCOMMAND>`: memory notes in a git vault.
//!
//! The vault and its index live in `lore-memory`; this module parses the
//! subcommand's arguments and prints. It never opens the archive.

use std::io::{Read, Write};
use std::path::PathBuf;

use lexopt::prelude::*;
use lore_memory::vault::{self, Added, Init, Vault};

use crate::cli::Invocation;
use crate::exit::{self, CliError};

/// The `mem` section of `--help`, also printed by `lorectl mem <SUB> --help`.
pub const USAGE: &str = "\
Memory (lorectl mem <SUBCOMMAND> [--vault <DIR>]):
    init              Create the vault, a git repository
    add <FILE>        Add a note file (`-` reads stdin); id, created, status,
                      and scope may be left out. The same file twice is one
                      note and one commit
    show <NOTE>       Print a note by id, slug, or vault path
    search <QUERY>    Rank notes by BM25 [--limit N, default 10]
    reindex           Rebuild the index from the vault files alone
    --vault <DIR>     Vault directory. Must be absolute
";

const DEFAULT_LIMIT: usize = 10;

#[derive(Debug, PartialEq, Eq)]
enum Sub {
    Init,
    Add(String),
    Show(String),
    Search(String),
    Reindex,
}

#[derive(Debug, PartialEq, Eq)]
struct MemArgs {
    sub: Option<Sub>,
    vault: Option<PathBuf>,
    limit: usize,
}

fn usage(message: impl Into<String>) -> CliError {
    CliError::Usage(message.into())
}

fn parse(sub: &str, rest: &[String]) -> Result<MemArgs, CliError> {
    let mut parser = lexopt::Parser::from_args(rest);
    let mut vault = None;
    let mut limit = None;
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
            Long("limit") if sub == "search" => {
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
        other => return Err(usage(format!("unknown mem subcommand `{other}`"))),
    };
    Ok(MemArgs {
        sub: Some(parsed),
        vault,
        limit: limit.unwrap_or(DEFAULT_LIMIT),
    })
}

/// Run `lorectl mem`.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let sub = invocation.operand.as_deref().unwrap_or_default();
    let args = parse(sub, &invocation.rest)?;
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
            let (note, added) = vault.add(&text)?;
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
            let width = hits.iter().map(|h| h.slug.len()).max().unwrap_or(0);
            let scope_width = hits.iter().map(|h| h.scope.len()).max().unwrap_or(0);
            for hit in hits {
                let _ = writeln!(
                    out,
                    "{:<width$}  {:<scope_width$}  {}",
                    hit.slug, hit.scope, hit.excerpt
                );
            }
        }
        Sub::Reindex => {
            let vault = Vault::open(&dir)?;
            let report = vault.reindex()?;
            for (rel, error) in &report.skipped {
                eprintln!("lorectl: skipped {rel}: {error}");
            }
            let _ = writeln!(
                out,
                "indexed {} notes, skipped {} files",
                report.indexed,
                report.skipped.len()
            );
        }
    }
    Ok(exit::OK)
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
        parse(sub, &rest)
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
