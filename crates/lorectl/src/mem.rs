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

/// The `mem` section of `--help`.
pub const USAGE: &str = "\
Memory (lorectl mem <SUBCOMMAND> [--vault <DIR>]):
    init              Create the vault, a git repository Obsidian can open
    add <FILE>        Add a note file (`-` reads stdin); id, created, status,
                      and scope may be left out. The same file twice is one
                      note and one commit. An id another note has is refused
    --vault <DIR>     Vault directory. Must be absolute
    Exit codes: 2 no vault, 5 vault busy, 7 note refused, 4 other failure.
";

#[derive(Debug, PartialEq, Eq)]
enum Sub {
    Init,
    Add(String),
}

#[derive(Debug, PartialEq, Eq)]
struct MemArgs {
    sub: Sub,
    vault: Option<PathBuf>,
}

fn usage(message: impl Into<String>) -> CliError {
    CliError::Usage(message.into())
}

fn parse(sub: &str, rest: &[String]) -> Result<MemArgs, CliError> {
    let mut parser = lexopt::Parser::from_args(rest);
    let mut vault = None;
    let mut operand: Option<String> = None;
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Long("vault") => {
                vault = Some(PathBuf::from(
                    parser.value().map_err(|e| usage(e.to_string()))?,
                ));
            }
            Value(value) if operand.is_none() => {
                operand = Some(value.to_string_lossy().into_owned());
            }
            other => return Err(usage(other.unexpected().to_string())),
        }
    }
    let needs = |name: &str| {
        operand
            .clone()
            .ok_or_else(|| usage(format!("`lorectl mem {sub}` needs a {name}")))
    };
    let parsed = match sub {
        "init" if operand.is_some() => {
            return Err(usage(format!("`lorectl mem {sub}` takes no argument")))
        }
        "init" => Sub::Init,
        "add" => Sub::Add(needs("FILE")?),
        other => return Err(usage(format!("unknown mem subcommand `{other}`"))),
    };
    Ok(MemArgs { sub: parsed, vault })
}

/// Run `lorectl mem`.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let sub = invocation.operand.as_deref().unwrap_or_default();
    let args = parse(sub, &invocation.rest)?;
    let dir = vault::resolve_dir(args.vault.as_deref())?;
    let mut out = std::io::stdout().lock();
    match args.sub {
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
        assert_eq!(args("init", &[]).unwrap().sub, Sub::Init);
        let add = args("add", &["n.md", "--vault", "/v"]).unwrap();
        assert_eq!(add.sub, Sub::Add("n.md".into()));
        assert_eq!(add.vault, Some(PathBuf::from("/v")));
    }

    #[test]
    fn bad_arguments_are_usage_errors() {
        for (sub, rest) in [
            ("frob", &[][..]),
            ("add", &[][..]),
            ("init", &["x"][..]),
            ("add", &["x", "--wat"][..]),
        ] {
            let error = args(sub, rest).unwrap_err();
            assert_eq!(exit::exit_code(&error), exit::USAGE, "{sub} {rest:?}");
        }
    }
}
