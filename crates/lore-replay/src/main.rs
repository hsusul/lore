//! `lore-replay`: extract correction points from the Lore archive, and replay
//! one under one arm. `run` starts Claude Code, which talks to a model
//! provider; `cases` only reads.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lexopt::prelude::*;
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_replay::grade::GradeOptions;
use lore_replay::label::LabelOptions;
use lore_replay::report::{self, Data};
use lore_replay::run::RunOptions;
use lore_replay::{cases, ArmName, Error};

const USAGE: &str = "\
lore-replay: replay Claude Code sessions from the point a person corrected the agent.

USAGE:
  lore-replay cases [--archive DIR] [--out FILE]
      Write one case per mined correction (JSON lines) to FILE, or stdout.
      Reads the archive read only. Prints the case count and each skip reason.

  lore-replay run --cases FILE --case ID --arm ARM --trial N --out DIR
                  [--arms FILE] [--model sonnet] [--max-turns 6] [--trim N] [--keep]
      Replay one case. Writes DIR/<case id, : as _>/<arm>-<trial>/run.json.
      ARM is none, own, file, lore, or oracle; file, lore, and oracle take their
      text from --arms (default: arms.json beside the cases file). Every arm
      and trial of a case runs in DIR/<case>/scratch with the same argv, so they
      share Claude Code's prompt cache; the arm's context goes in a
      <system-reminder> block before the prompt. Runs of one case wait for each
      other (a lock on DIR/<case>/scratch.lock).
      --trim N keeps the last N user and assistant lines of the replayed
      history whole and, in every earlier one, replaces each tool result's
      output with \"[tool output omitted in replay: <length> characters]\" and
      each image with \"[image omitted in replay]\". Tool call ids, assistant
      text, and tool calls are kept. 0, the default, trims nothing. run.json
      records model, trim, and max_turns.
      This starts Claude Code, which sends the session to the model provider.
      Set LORE_REPLAY_CLAUDE to use a claude other than the one on PATH.

  lore-replay sweep --cases FILE (--case ID [--case ID ...] | --case-list FILE)
                    --arms FILE --out DIR [--trials 3] [--jobs 4] [--model sonnet]
                    [--max-turns 6] [--trim N] [--run-arms LIST] [--max-cost USD]
                    [--judge-model sonnet]
      Replay many cases under every arm. --cases is a labeled file (label's
      output); --case-list names one case id per line. Cases run JOBS at a time,
      the runs of a case one after another, so they share the prompt cache.
      Each case is screened first: none trial 0 runs and is graded (the label's
      deterministic grader, else the judge). A case that does not repeat the
      mistake stops there, recorded in DIR/screen.jsonl; one that does runs none,
      own, file, lore, and oracle trials 1..TRIALS. file or lore with no text is
      skipped as same as none; a missing oracle text is an error before any run.
      --run-arms LIST (comma-separated: none, own, file, lore, oracle) runs only
      those arms; none always runs, as the screen and the baseline, and the arms
      run in the order above. --trim is as for run. Runs that already succeeded
      under the same harness, model, trim, and turn cap are kept, so rerunning
      resumes; give each setting its own DIR. Stops the
      whole sweep, with exit code 3, when a run hits the usage limit, and with
      exit code 4 when this sweep's runs cost more than --max-cost.
      This starts Claude Code and the judge, which talk to the model provider.

  lore-replay label --cases FILE --out FILE [--model sonnet] [--batch 8]
      Ask a model judge, BATCH cases at a time, whether each corrected turn made
      a mistake, what it was, the lesson, and how to grade a repeat. Appends one
      labeled case per line to FILE; cases already labeled there are skipped.
      This sends the cases to the model provider through claude -p.

  lore-replay grade --labels FILE --runs DIR --out FILE [--model sonnet]
                    [--no-judge] [--arms FILE --shown-out FILE]
      Grade every DIR/*/*/run.json: a deterministic verdict when the label
      names a deterministic grader, and a model-judge verdict on every run
      (none with --no-judge). Errored and empty runs are skipped and counted;
      runs already graded in FILE are skipped. With --shown-out, also judge once
      per case and arm (file, lore, own) whether the arm's context holds a
      lesson for the case's mistake; own is today's CLAUDE.md unless --arms
      records an `own` text.

  lore-replay report --labels FILE --grades FILE --data real|synthetic
                     [--shown FILE] [--arms FILE] [--screen FILE] [--harness N]
                     [--settings KEY=VALUE,...] [--json] [--seed N]
      The preregistered analysis: per-arm case-weighted repeat rates and paired
      differences against none, with 95% cluster-bootstrap intervals over cases,
      the stop rule, spread, grader agreement, oracle validity, and (with
      --shown) exists/shown/followed. --arms marks arms with empty context as
      same as none. Trial 0, sweep's screening run, is left out of every rate,
      so rates are conditional on cases that reproduced at screening; --screen
      (sweep's screen.jsonl) reports how many cases did not. Runs are grouped by
      their settings (harness, model, trim, max_turns, which grade copies from
      run.json), and grades from more than one group are refused unless
      --settings picks one, e.g. --settings model=haiku,trim=20,max_turns=6
      (keys harness, model, trim, max_turns; `unrecorded` matches a grade that
      does not say). --harness N is --settings harness=N. The header names the
      group reported. Every number is tagged with --data. Reads only.
";

enum Failure {
    Usage(String),
    Run(Error),
    /// A sweep stopped on purpose: the message and the exit code.
    Stopped(String, u8),
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
        Err(Failure::Stopped(message, code)) => {
            eprintln!("lore-replay: {message}");
            ExitCode::from(code)
        }
    }
}

fn dispatch() -> Result<(), Failure> {
    let mut parser = lexopt::Parser::from_env();
    match parser.next()? {
        Some(Value(cmd)) if cmd == "cases" => cases_cmd(&mut parser),
        Some(Value(cmd)) if cmd == "run" => run_cmd(&mut parser),
        Some(Value(cmd)) if cmd == "sweep" => sweep_cmd(&mut parser),
        Some(Value(cmd)) if cmd == "label" => label_cmd(&mut parser),
        Some(Value(cmd)) if cmd == "grade" => grade_cmd(&mut parser),
        Some(Value(cmd)) if cmd == "report" => report_cmd(&mut parser),
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
    let (mut model, mut max_turns, mut keep_scratch, mut trim) = (
        "sonnet".to_owned(),
        lore_replay::run::DEFAULT_MAX_TURNS,
        false,
        0_u32,
    );
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
            Long("trim") => trim = parser.value()?.parse::<u32>()?,
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
        trim,
        keep_scratch,
    };
    let record = lore_replay::run::run(&case, &arm, &opts)?;
    println!("{}", lore_replay::run::progress(&record));
    Ok(())
}

fn sweep_cmd(parser: &mut lexopt::Parser) -> Result<(), Failure> {
    let (mut cases_file, mut case_list, mut arms, mut out, mut max_cost) =
        (None, None, None, None, None);
    let mut case_ids = Vec::new();
    let (mut trials, mut jobs, mut max_turns, mut trim) =
        (3_u32, 4_usize, lore_replay::run::DEFAULT_MAX_TURNS, 0_u32);
    let mut arms_to_run = ArmName::ALL.to_vec();
    let (mut model, mut judge_model) = ("sonnet".to_owned(), "sonnet".to_owned());
    while let Some(arg) = parser.next()? {
        match arg {
            Long("cases") => cases_file = Some(path(parser)?),
            Long("case") => case_ids.push(text(parser)?),
            Long("case-list") => case_list = Some(path(parser)?),
            Long("arms") => arms = Some(path(parser)?),
            Long("out") => out = Some(path(parser)?),
            Long("trials") => trials = parser.value()?.parse::<u32>()?,
            Long("jobs") => jobs = parser.value()?.parse::<usize>()?,
            Long("model") => model = text(parser)?,
            Long("max-turns") => max_turns = parser.value()?.parse::<u32>()?,
            Long("trim") => trim = parser.value()?.parse::<u32>()?,
            Long("run-arms") => {
                arms_to_run = text(parser)?
                    .split(',')
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .map(str::parse::<ArmName>)
                    .collect::<Result<_, _>>()
                    .map_err(Failure::Usage)?;
            }
            Long("max-cost") => max_cost = Some(parser.value()?.parse::<f64>()?),
            Long("judge-model") => judge_model = text(parser)?,
            Short('h') | Long("help") => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(Failure::Usage(other.unexpected().to_string())),
        }
    }
    let need = |flag: &str| Failure::Usage(format!("{flag} is required"));
    match (&case_list, case_ids.is_empty()) {
        (Some(list), true) => {
            let text = std::fs::read_to_string(list).map_err(|e| Error::io("read", list, e))?;
            case_ids.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(str::to_owned),
            );
        }
        (Some(_), false) => {
            return Err(Failure::Usage(
                "pass --case or --case-list, not both".into(),
            ))
        }
        (None, true) => return Err(need("--case or --case-list")),
        (None, false) => {}
    }
    if trials == 0 || jobs == 0 {
        return Err(Failure::Usage(
            "--trials and --jobs must be at least 1".into(),
        ));
    }
    if max_cost.is_some_and(|c: f64| !c.is_finite() || c < 0.0) {
        return Err(Failure::Usage(
            "--max-cost must be a non-negative number".into(),
        ));
    }
    let opts = lore_replay::sweep::SweepOptions {
        cases: cases_file.ok_or_else(|| need("--cases"))?,
        case_ids,
        arms: arms.ok_or_else(|| need("--arms"))?,
        out: out.ok_or_else(|| need("--out"))?,
        trials,
        jobs,
        model,
        max_turns,
        trim,
        max_cost,
        judge_model,
        arms_to_run,
    };
    let summary = lore_replay::sweep::sweep(&opts)?;
    println!("{}", summary.summary());
    match &summary.stop {
        Some(stop) => Err(Failure::Stopped(stop.to_string(), stop.exit_code())),
        None => Ok(()),
    }
}

fn label_cmd(parser: &mut lexopt::Parser) -> Result<(), Failure> {
    let (mut cases_file, mut out) = (None, None);
    let (mut model, mut batch) = ("sonnet".to_owned(), 8_usize);
    while let Some(arg) = parser.next()? {
        match arg {
            Long("cases") => cases_file = Some(path(parser)?),
            Long("out") => out = Some(path(parser)?),
            Long("model") => model = text(parser)?,
            Long("batch") => batch = parser.value()?.parse::<usize>()?,
            Short('h') | Long("help") => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(Failure::Usage(other.unexpected().to_string())),
        }
    }
    let need = |flag: &str| Failure::Usage(format!("{flag} is required"));
    if batch == 0 {
        return Err(Failure::Usage("--batch must be at least 1".into()));
    }
    let opts = LabelOptions {
        cases: cases_file.ok_or_else(|| need("--cases"))?,
        out: out.ok_or_else(|| need("--out"))?,
        model,
        batch,
    };
    println!("{}", lore_replay::label::label(&opts)?.summary());
    Ok(())
}

fn grade_cmd(parser: &mut lexopt::Parser) -> Result<(), Failure> {
    let (mut labels, mut runs, mut out, mut arms, mut shown_out) = (None, None, None, None, None);
    let (mut model, mut judge) = ("sonnet".to_owned(), true);
    while let Some(arg) = parser.next()? {
        match arg {
            Long("labels") => labels = Some(path(parser)?),
            Long("runs") => runs = Some(path(parser)?),
            Long("out") => out = Some(path(parser)?),
            Long("model") => model = text(parser)?,
            Long("no-judge") => judge = false,
            Long("arms") => arms = Some(path(parser)?),
            Long("shown-out") => shown_out = Some(path(parser)?),
            Short('h') | Long("help") => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(Failure::Usage(other.unexpected().to_string())),
        }
    }
    let need = |flag: &str| Failure::Usage(format!("{flag} is required"));
    let opts = GradeOptions {
        labels: labels.ok_or_else(|| need("--labels"))?,
        runs: runs.ok_or_else(|| need("--runs"))?,
        out: out.ok_or_else(|| need("--out"))?,
        model,
        judge,
        arms,
        shown_out,
    };
    println!("{}", lore_replay::grade::grade(&opts)?.summary());
    Ok(())
}

fn report_cmd(parser: &mut lexopt::Parser) -> Result<(), Failure> {
    let (mut labels, mut grades, mut shown, mut arms, mut data) = (None, None, None, None, None);
    let mut screen = None;
    let mut settings = report::SettingsPick::default();
    let (mut json, mut seed) = (false, report::DEFAULT_SEED);
    while let Some(arg) = parser.next()? {
        match arg {
            Long("labels") => labels = Some(path(parser)?),
            Long("grades") => grades = Some(path(parser)?),
            Long("shown") => shown = Some(path(parser)?),
            Long("arms") => arms = Some(path(parser)?),
            Long("screen") => screen = Some(path(parser)?),
            Long("harness") => settings.harness = Some(parser.value()?.parse::<u32>()?),
            Long("settings") => {
                let pick = text(parser)?
                    .parse::<report::SettingsPick>()
                    .map_err(Failure::Usage)?;
                settings = report::SettingsPick {
                    harness: pick.harness.or(settings.harness),
                    ..pick
                };
            }
            Long("data") => {
                data = Some(text(parser)?.parse::<Data>().map_err(Failure::Usage)?);
            }
            Long("json") => json = true,
            Long("seed") => seed = parser.value()?.parse::<u64>()?,
            Short('h') | Long("help") => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(Failure::Usage(other.unexpected().to_string())),
        }
    }
    let need = |flag: &str| Failure::Usage(format!("{flag} is required"));
    let opts = report::ReportOptions {
        labels: labels.ok_or_else(|| need("--labels"))?,
        grades: grades.ok_or_else(|| need("--grades"))?,
        shown,
        arms,
        screen,
        settings,
        data: data.ok_or_else(|| need("--data"))?,
        seed,
    };
    let result = report::analyze(&report::load(&opts)?, opts.data, opts.seed);
    if json {
        let text = serde_json::to_string_pretty(&result).map_err(|source| Error::Json {
            path: opts.grades.clone(),
            source,
        })?;
        println!("{text}");
    } else {
        print!("{}", result.text());
    }
    Ok(())
}

fn beside(file: &Path, name: &str) -> PathBuf {
    file.parent().unwrap_or(Path::new("")).join(name)
}
