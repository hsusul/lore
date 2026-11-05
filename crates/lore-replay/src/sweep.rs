//! `sweep`: replay many cases under every arm, cheaply.
//!
//! Cases run in parallel, up to `jobs` at a time; the runs of one case run
//! one after another, so the first writes the prompt cache and the rest read
//! it (see [`crate::run`]).
//!
//! Each case is screened first: `none` trial 0 runs and is graded with the
//! label's deterministic grader, or the judge when the label has none. A case
//! whose screening run does not repeat the mistake cannot show an arm
//! preventing it, so it stops there, and `<out>/screen.jsonl` says why. A case
//! that does repeat runs `none` trials 1..N, then `own`, `file`, `lore`, and
//! `oracle` trials 1..N. `file` or `lore` with an empty or missing text is
//! skipped and recorded as the same as `none`. Trial 0 is the screening run:
//! the report leaves it out of every rate.
//!
//! A sweep resumes: a run whose `run.json` succeeded under the current
//! harness is not run again, and a case already screened out stays out.
//!
//! Before every run it checks two stop conditions, and either ends the whole
//! sweep: a run hit the subscription's usage limit (one did, and the next 60
//! returned the same error at once), or the runs this sweep made cost more
//! than `max_cost`.
//!
//! **This starts Claude Code and the judge, which talk to a model provider.**

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::grade::{self, Judging, RunView};
use crate::label::{append, read_lines};
use crate::run::{self, RunOptions};
use crate::types::{
    Arm, ArmName, ArmsFile, Case, LabeledCase, RunRecord, ScreenLine, Verdict, HARNESS,
};

#[derive(Debug, Clone)]
pub struct SweepOptions {
    /// Cases with labels: each line a case plus `label`.
    pub cases: PathBuf,
    pub case_ids: Vec<String>,
    pub arms: PathBuf,
    pub out: PathBuf,
    /// Trials per arm after screening: 1..=trials.
    pub trials: u32,
    pub jobs: usize,
    pub model: String,
    pub max_turns: u32,
    pub max_cost: Option<f64>,
    pub judge_model: String,
}

/// Why a sweep ended early.
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    UsageLimit { run: String, text: String },
    MaxCost { spent: f64, max: f64 },
}

impl Stop {
    /// The exit code `lore-replay sweep` ends with.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::UsageLimit { .. } => 3,
            Self::MaxCost { .. } => 4,
        }
    }
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UsageLimit { run, text } => write!(
                f,
                "sweep stopped: {run} hit the usage limit ({text}). Rerun the same command after the limit resets; finished runs are kept"
            ),
            Self::MaxCost { spent, max } => write!(
                f,
                "sweep stopped: this sweep's runs cost ${spent:.2}, over --max-cost ${max:.2}. Rerun with a higher --max-cost to continue; finished runs are kept"
            ),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SweepSummary {
    /// Runs this sweep started.
    pub runs_made: usize,
    /// Runs already done, not run again.
    pub runs_kept: usize,
    /// Cases screened (now or before) whose screening run did not repeat.
    pub screened_out: usize,
    /// Cases screened (now or before) whose screening run repeated.
    pub screened_in: usize,
    /// Cases whose screening run errored; a rerun tries them again.
    pub screen_errors: usize,
    /// Summed `cost_usd` of the runs this sweep made.
    pub cost_usd: f64,
    pub stop: Option<Stop>,
}

impl SweepSummary {
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "sweep: {} runs made, {} already done; {} of {} screened cases screened out, {} screening runs errored; ${:.2} spent",
            self.runs_made,
            self.runs_kept,
            self.screened_out,
            self.screened_out + self.screened_in,
            self.screen_errors,
            self.cost_usd
        )
    }
}

/// One case, ready to sweep: what it needs is checked before anything runs.
struct Plan {
    case: Case,
    labeled: LabeledCase,
    /// The arms after screening, in run order.
    arms: Vec<Arm>,
    /// `file` / `lore` with no text: skipped as the same as `none`.
    same_as_none: Vec<ArmName>,
}

enum Halt {
    Stop(Stop),
    Failed(Error),
}

struct Shared<'a> {
    opts: &'a SweepOptions,
    halt: Mutex<Option<Halt>>,
    tally: Mutex<SweepSummary>,
    screen: Mutex<std::fs::File>,
    screen_path: PathBuf,
    prior_verdicts: HashMap<String, Verdict>,
    prior_same: HashSet<(String, ArmName)>,
}

/// `<out>/screen.jsonl`.
#[must_use]
pub fn screen_file(out: &Path) -> PathBuf {
    out.join("screen.jsonl")
}

/// Run the sweep. `Ok` with [`SweepSummary::stop`] set when a stop condition
/// ended it; `Err` when a run or the judge failed outright.
pub fn sweep(opts: &SweepOptions) -> Result<SweepSummary> {
    let plans = plan(opts)?;
    let screen_path = screen_file(&opts.out);
    let prior: Vec<ScreenLine> = read_lines(&screen_path)?;
    let prior_verdicts = prior
        .iter()
        .filter_map(|l| Some((l.case.clone(), l.verdict?)))
        .collect();
    let prior_same = prior
        .iter()
        .filter(|l| l.same_as_none)
        .filter_map(|l| Some((l.case.clone(), l.arm?)))
        .collect();
    let shared = Shared {
        opts,
        halt: Mutex::new(None),
        tally: Mutex::new(SweepSummary::default()),
        screen: Mutex::new(append(&screen_path)?),
        screen_path,
        prior_verdicts,
        prior_same,
    };
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..opts.jobs.clamp(1, plans.len().max(1)) {
            scope.spawn(|| loop {
                if shared.halted() {
                    break;
                }
                let Some(plan) = plans.get(next.fetch_add(1, Ordering::SeqCst)) else {
                    break;
                };
                if let Err(e) = shared.sweep_case(plan) {
                    shared.halt_with(Halt::Failed(e));
                }
            });
        }
    });
    let mut summary = lock(&shared.tally).clone();
    match shared.halt.into_inner().unwrap_or_else(|p| p.into_inner()) {
        Some(Halt::Failed(e)) => Err(e),
        Some(Halt::Stop(stop)) => {
            summary.stop = Some(stop);
            Ok(summary)
        }
        None => Ok(summary),
    }
}

/// Read the cases and arms and check every requested case: it exists, it
/// has a label, and it has an oracle text.
fn plan(opts: &SweepOptions) -> Result<Vec<Plan>> {
    let text =
        std::fs::read_to_string(&opts.cases).map_err(|e| Error::io("read", &opts.cases, e))?;
    let json = |source| Error::Json {
        path: opts.cases.clone(),
        source,
    };
    let mut lines: HashMap<String, Value> = HashMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let value: Value = serde_json::from_str(line).map_err(json)?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        lines.insert(id, value);
    }
    let arms_text =
        std::fs::read_to_string(&opts.arms).map_err(|e| Error::io("read", &opts.arms, e))?;
    let arms: ArmsFile = serde_json::from_str(&arms_text).map_err(|source| Error::Json {
        path: opts.arms.clone(),
        source,
    })?;
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for id in opts.case_ids.iter().filter(|id| seen.insert(id.as_str())) {
        let value = lines
            .get(id)
            .ok_or_else(|| Error::CaseNotFound(id.clone()))?;
        let case: Case = serde_json::from_value(value.clone()).map_err(json)?;
        let labeled: LabeledCase = serde_json::from_value(value.clone()).map_err(json)?;
        if labeled.label.is_none() {
            return Err(Error::Unlabeled(id.clone()));
        }
        let text = |arm: ArmName| arms.get(id).and_then(|a| a.get(arm.as_str())).cloned();
        let oracle = text(ArmName::Oracle).ok_or_else(|| Error::ArmTextMissing {
            case: id.clone(),
            arm: ArmName::Oracle.to_string(),
        })?;
        let mut run_arms = vec![Arm::None, Arm::Own];
        let mut same_as_none = Vec::new();
        for name in [ArmName::File, ArmName::Lore] {
            match text(name).filter(|t| !t.trim().is_empty()) {
                Some(t) => run_arms.extend(Arm::new(name, Some(t))),
                None => same_as_none.push(name),
            }
        }
        run_arms.push(Arm::Oracle(oracle));
        plans.push(Plan {
            case,
            labeled,
            arms: run_arms,
            same_as_none,
        });
    }
    Ok(plans)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Shared<'_> {
    fn halted(&self) -> bool {
        lock(&self.halt).is_some()
    }

    /// The first halt wins.
    fn halt_with(&self, halt: Halt) {
        let mut slot = lock(&self.halt);
        if slot.is_none() {
            *slot = Some(halt);
        }
    }

    /// Check the stop conditions before a run starts.
    fn may_start(&self) -> bool {
        if self.halted() {
            return false;
        }
        let spent = lock(&self.tally).cost_usd;
        match self.opts.max_cost {
            Some(max) if spent > max => {
                self.halt_with(Halt::Stop(Stop::MaxCost { spent, max }));
                false
            }
            _ => true,
        }
    }

    fn append_screen(&self, line: &ScreenLine) -> Result<()> {
        grade::write_line(&mut lock(&self.screen), &self.screen_path, line)?;
        println!(
            "{}",
            serde_json::to_string(line).unwrap_or_else(|_| line.case.clone())
        );
        Ok(())
    }

    fn sweep_case(&self, plan: &Plan) -> Result<()> {
        let id = &plan.case.id;
        let prior = self.prior_verdicts.get(id).copied();
        if prior.is_some_and(|v| v != Verdict::Yes) {
            lock(&self.tally).screened_out += 1;
            return Ok(());
        }
        let Some((zero, fresh)) = self.run_or_keep(plan, &Arm::None, 0)? else {
            return Ok(());
        };
        if zero.is_error != Some(false) {
            eprintln!("lore-replay: the screening run of {id} errored; a rerun tries it again");
            lock(&self.tally).screen_errors += 1;
            return Ok(());
        }
        let verdict = match prior {
            Some(v) if !fresh => v,
            _ => {
                let (verdict, reason) = self.screen(plan, &zero)?;
                self.append_screen(&ScreenLine {
                    case: id.clone(),
                    verdict: Some(verdict),
                    arm: None,
                    same_as_none: false,
                    reason,
                })?;
                verdict
            }
        };
        if verdict != Verdict::Yes {
            lock(&self.tally).screened_out += 1;
            return Ok(());
        }
        lock(&self.tally).screened_in += 1;
        for &name in &plan.same_as_none {
            if !self.prior_same.contains(&(id.clone(), name)) {
                self.append_screen(&ScreenLine {
                    case: id.clone(),
                    verdict: None,
                    arm: Some(name),
                    same_as_none: true,
                    reason: format!("the arms file has no {name} text; same as none"),
                })?;
            }
        }
        for arm in &plan.arms {
            for trial in 1..=self.opts.trials {
                if self.run_or_keep(plan, arm, trial)?.is_none() {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// The screening verdict for trial 0 and its reason.
    fn screen(&self, plan: &Plan, zero: &RunRecord) -> Result<(Verdict, String)> {
        let value = serde_json::to_value(zero).map_err(|source| Error::Json {
            path: self.screen_path.clone(),
            source,
        })?;
        let view: RunView = serde_json::from_value(value).map_err(|source| Error::Json {
            path: self.screen_path.clone(),
            source,
        })?;
        let line = grade::grade_run(
            &plan.labeled,
            &view,
            Some(&self.opts.judge_model),
            Judging::WithoutDeterministic,
        )?;
        let verdict = match line.repeat() {
            Some(true) => Verdict::Yes,
            Some(false) => Verdict::No,
            None => Verdict::Unknown,
        };
        let reason = match (&line.judge_reason, line.repeat_det) {
            (Some(reason), _) => reason.clone(),
            (None, Some(det)) => format!(
                "the {} grader says the screening run {} the mistake",
                line.grader.as_deref().unwrap_or("deterministic"),
                if det { "repeats" } else { "does not repeat" }
            ),
            (None, None) => "no verdict".to_owned(),
        };
        Ok((verdict, reason))
    }

    /// The record of `arm` trial `trial`: the kept one when it succeeded under
    /// the current harness, else a new run. `None` when a stop condition holds
    /// or the new run hit the usage limit. The bool is true for a new run.
    fn run_or_keep(&self, plan: &Plan, arm: &Arm, trial: u32) -> Result<Option<(RunRecord, bool)>> {
        let opts = RunOptions {
            out: self.opts.out.clone(),
            trial,
            model: self.opts.model.clone(),
            max_turns: self.opts.max_turns,
            keep_scratch: false,
        };
        if let Some(kept) = kept(&run::run_dir(&plan.case, arm, &opts).join("run.json")) {
            lock(&self.tally).runs_kept += 1;
            return Ok(Some((kept, false)));
        }
        if !self.may_start() {
            return Ok(None);
        }
        let record = run::run(&plan.case, arm, &opts)?;
        println!("{}", run::progress(&record));
        {
            let mut tally = lock(&self.tally);
            tally.runs_made += 1;
            tally.cost_usd += record.cost_usd.unwrap_or(0.0);
        }
        if run::usage_limited(&record) {
            let text = run::result_event(&record.events)
                .and_then(|r| r.get("result"))
                .and_then(Value::as_str)
                .unwrap_or("an error with no turns and no cost");
            self.halt_with(Halt::Stop(Stop::UsageLimit {
                run: format!("{} {}-{}", record.case, record.arm, record.trial),
                text: crate::claude::head(text, 200),
            }));
            return Ok(None);
        }
        Ok(Some((record, true)))
    }
}

/// A finished run worth keeping: it succeeded, under the current harness.
fn kept(path: &Path) -> Option<RunRecord> {
    let text = std::fs::read_to_string(path).ok()?;
    let record: RunRecord = serde_json::from_str(&text).ok()?;
    (record.is_error == Some(false) && record.harness == HARNESS).then_some(record)
}
