//! `lore-replay`: extract correction points from the Lore archive, and replay
//! one under one arm. `run` starts Claude Code, which talks to a model
//! provider; `cases` only reads.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lexopt::prelude::*;
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_replay::run::RunOptions;
use lore_replay::{cases, ArmName, Error};

const USAGE: &str = "\
lore-replay: replay Claude Code sessions from the point a person corrected the agent.

USAGE:
  lore-replay cases [--archive DIR] [--out FILE]
      Write one case per mined correction (JSON lines) to FILE, or stdout.
      Reads the archive read only. Prints the case count and each skip reason.

  lore-replay run --cases FILE --case ID --arm ARM --trial N --out DIR
                  [--arms FILE] [--model sonnet] [--max-turns 25] [--keep]
      Replay one case. Writes DIR/<case id, : as _>/<arm>-<trial>/run.json.
      ARM is none, own, file, lore, or oracle; file, lore, and oracle take their
      text from --arms (default: arms.json beside the cases file).
      This starts Claude Code, which sends the session to the model provider.
      Set LORE_REPLAY_CLAUDE to use a claude other than the one on PATH.
";

enum Failure {
    Usage(String),
    Run(Error),
}

impl From<lexopt::Error> for Failure {
    fn from(e: lexopt::Error) -> Self {
        Self::Usage(e.to_string())
    }
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Self::Run(e)
    }
}

fn main() -> ExitCode {
    match dispatch() {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            eprintln!("lore-replay: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Run(error)) => {
            eprintln!("lore-replay: {error}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch() -> Result<(), Failure> {
    let mut parser = lexopt::Parser::from_env();
    match parser.next()? {
        Some(Value(cmd)) if cmd == "cases" => cases_cmd(&mut parser),
        Some(Value(cmd)) if cmd == "run" => run_cmd(&mut parser),
        Some(Short('h') | Long("help")) => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(Failure::Usage(other.unexpected().to_string())),
        None => Err(Failure::Usage("a command is required".into())),
    }
}

fn path(parser: &mut lexopt::Parser) -> Result<PathBuf, Failure> {
    Ok(PathBuf::from(parser.value()?))
}

fn text(parser: &mut lexopt::Parser) -> Result<String, Failure> {
    Ok(parser.value()?.string()?)
}

fn cases_cmd(parser: &mut lexopt::Parser) -> Result<(), Failure> {
    let (mut archive, mut out) = (None, None);
    while let Some(arg) = parser.next()? {
        match arg {
            Long("archive") => archive = Some(path(parser)?),
            Long("out") => out = Some(path(parser)?),
            Short('h') | Long("help") => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(Failure::Usage(other.unexpected().to_string())),
        }
    }
    let db = lore_core::paths::archive_dir(archive.as_deref())
        .map_err(Error::from)?
        .join(ARCHIVE_DB_FILENAME);
    let conn = lore_core::storage::open_read_only(&db).map_err(Error::from)?;
    let projects = cases::claude_projects_dir().ok_or(Error::NoHome)?;
    let found = cases::extract(&conn, &projects)?;
    let mut lines = String::new();
    for case in &found.cases {
        let json = serde_json::to_string(case).map_err(|source| Error::Json {
            path: out.clone().unwrap_or_default(),
            source,
        })?;
        lines.push_str(&json);
        lines.push('\n');
    }
    match &out {
        Some(file) => {
            if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
            }
            std::fs::write(file, lines).map_err(|e| Error::io("write", file, e))?;
            println!("{}", found.summary());
        }
        None => {
            let _ = std::io::stdout().lock().write_all(lines.as_bytes());
            eprintln!("{}", found.summary());
        }
    }
    Ok(())
}

fn run_cmd(parser: &mut lexopt::Parser) -> Result<(), Failure> {
    let (mut cases_file, mut case, mut arm, mut trial, mut out, mut arms) =
        (None, None, None, None, None, None);
    let (mut model, mut max_turns, mut keep_scratch) = ("sonnet".to_owned(), 25_u32, false);
    while let Some(arg) = parser.next()? {
        match arg {
            Long("cases") => cases_file = Some(path(parser)?),
            Long("case") => case = Some(text(parser)?),
            Long("arm") => {
                arm = Some(text(parser)?.parse::<ArmName>().map_err(Failure::Usage)?);
            }
            Long("trial") => trial = Some(parser.value()?.parse::<u32>()?),
            Long("out") => out = Some(path(parser)?),
            Long("arms") => arms = Some(path(parser)?),
            Long("model") => model = text(parser)?,
            Long("max-turns") => max_turns = parser.value()?.parse::<u32>()?,
            Long("keep") => keep_scratch = true,
            Short('h') | Long("help") => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(Failure::Usage(other.unexpected().to_string())),
        }
    }
    let need = |flag: &str| Failure::Usage(format!("{flag} is required"));
    let cases_file = cases_file.ok_or_else(|| need("--cases"))?;
    let case_id = case.ok_or_else(|| need("--case"))?;
    let arm_name = arm.ok_or_else(|| need("--arm"))?;
    let trial = trial.ok_or_else(|| need("--trial"))?;
    let out = out.ok_or_else(|| need("--out"))?;
    let arms = arms.or_else(|| Some(beside(&cases_file, "arms.json")));

    let case = lore_replay::find_case(&cases_file, &case_id)?;
    let arm = lore_replay::load_arm(arms.as_deref(), &case.id, arm_name)?;
    let opts = RunOptions {
        out,
        trial,
        model,
        max_turns,
        keep_scratch,
    };
    let record = lore_replay::run::run(&case, &arm, &opts)?;
    println!(
        "{}",
        serde_json::json!({
            "case": record.case, "arm": record.arm, "trial": record.trial,
            "fidelity": record.fidelity, "seconds": record.seconds,
            "cost_usd": record.cost_usd, "is_error": record.is_error,
        })
    );
    Ok(())
}

fn beside(file: &Path, name: &str) -> PathBuf {
    file.parent().unwrap_or(Path::new("")).join(name)
}
