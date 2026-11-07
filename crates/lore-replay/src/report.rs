//! `report`: the preregistered analysis (`eval/PREREGISTRATION.md`) of graded
//! replays. Reads only; it never starts a model.
//!
//! The rules, in the order the report applies them:
//!
//! - Grades from one group of settings only: harness version (see
//!   [`crate::types::HARNESS`]), model, trim, and turn cap. Mixed groups are
//!   refused unless a [`SettingsPick`] narrows them to one, and the group
//!   reported is printed in the header.
//! - Trial 0 is `sweep`'s screening run, which decides whether a case runs
//!   at all, so it is left out of every rate: the rates are conditional on
//!   the cases that reproduced at screening.
//! - A case counts only when its label says `mistake` and `visible`.
//! - A run's verdict is the deterministic one when it exists, else the
//!   judge's. UNKNOWN and unjudged runs are excluded and counted.
//! - When an arm's context is empty for a case, that arm *is* `none` for the
//!   case: it is reported as "same as none" and uses the `none` trials.
//! - Per arm: the case-weighted mean repeat rate (each case counts once,
//!   whatever its trial count), with a 95% interval from a cluster bootstrap
//!   over cases. Trials within a case are correlated, so cases, not trials,
//!   are what is resampled.
//! - Against `none`: the mean over cases of (arm − none), same interval. An
//!   arm whose interval contains 0, or that pairs with fewer than two cases,
//!   prints "no measurable difference from none" and no percentage.
//! - Stop rule, printed first: when `none` repeats the mistake in fewer than
//!   one third of its graded trials, the replay cannot measure the archive
//!   and the report prints no percentages at all.
//!
//! Every number is tagged with where it was measured (`--data`), and every
//! rate carries its n.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::str::FromStr;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::label::{read_labels, read_lines};
use crate::types::{Agent, ArmName, ArmsFile, GradeLine, ScreenLine, ShownLine, Verdict};

/// The bootstrap's seed unless `--seed` says otherwise: the preregistration
/// date, so a rerun prints the same intervals.
pub const DEFAULT_SEED: u64 = 20_261_105;
/// Cluster-bootstrap resamples per interval.
pub const RESAMPLES: usize = 10_000;
/// The stop rule's threshold on the `none` arm's pooled repeat rate.
pub const MIN_NONE_RATE: f64 = 1.0 / 3.0;

/// Where the numbers come from. Printed beside every number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Data {
    Real,
    Synthetic,
}

impl Data {
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Real => "real data",
            Self::Synthetic => "synthetic data",
        }
    }
}

impl FromStr for Data {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "real" => Ok(Self::Real),
            "synthetic" => Ok(Self::Synthetic),
            other => Err(format!(
                "unknown --data `{other}`; expected real or synthetic"
            )),
        }
    }
}

/// A deterministic generator (SplitMix64), so a seed fixes every interval.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; `n` must be positive.
    pub fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// The 95% percentile interval of the mean of `values` (one value per
/// case), over `resamples` resamples of the cases with replacement.
#[must_use]
pub fn bootstrap_ci(values: &[f64], resamples: usize, seed: u64) -> Option<(f64, f64)> {
    if values.is_empty() || resamples == 0 {
        return None;
    }
    let mut rng = Rng::new(seed);
    let n = values.len();
    let mut means: Vec<f64> = (0..resamples)
        .map(|_| (0..n).map(|_| values[rng.below(n)]).sum::<f64>() / n as f64)
        .collect();
    means.sort_by(f64::total_cmp);
    let lo = ((resamples as f64) * 0.025).floor() as usize;
    let hi = (((resamples as f64) * 0.975).ceil() as usize).saturating_sub(1);
    Some((means[lo.min(resamples - 1)], means[hi.min(resamples - 1)]))
}

/// What the analysis reads, already parsed.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Every labeled case, included or not.
    pub labeled: usize,
    /// The included cases: id and the label's `what`.
    pub cases: Vec<(String, String)>,
    pub grades: Vec<GradeLine>,
    pub shown: Vec<ShownLine>,
    /// (case, arm) pairs whose context is empty.
    pub empty: HashSet<(String, ArmName)>,
    /// From a screen file: cases that did not reproduce (`k`) of the cases
    /// screened (`n`).
    pub screened_out: Option<Count>,
    /// The settings every grade shares; `None` without grades.
    pub settings: Option<Settings>,
}

/// The settings a group of runs shares. Runs with different settings measure
/// different things, so a report covers one group.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Settings {
    /// The agent replayed. Claude Code and Codex runs never share a report.
    pub agent: Agent,
    pub harness: u32,
    /// `None` when the grade line does not say (graded before it was copied).
    pub model: Option<String>,
    pub trim: u32,
    /// `None` when the run did not record it.
    pub max_turns: Option<u32>,
}

const UNRECORDED: &str = "unrecorded";

impl Settings {
    #[must_use]
    pub fn of(grade: &GradeLine) -> Self {
        Self {
            agent: grade.agent,
            harness: grade.harness_version(),
            model: grade.model.clone(),
            trim: grade.trim.unwrap_or(0),
            max_turns: grade.max_turns,
        }
    }

    fn model_str(&self) -> String {
        self.model.clone().unwrap_or_else(|| UNRECORDED.to_owned())
    }

    fn max_turns_str(&self) -> String {
        self.max_turns
            .map_or_else(|| UNRECORDED.to_owned(), |m| m.to_string())
    }
}

impl std::fmt::Display for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "agent {}, harness {}, model {}, trim {}, max turns {}",
            self.agent,
            self.harness,
            self.model_str(),
            self.trim,
            self.max_turns_str()
        )
    }
}

/// `--settings KEY=VALUE,...`: keep the grades whose settings match every
/// key given. Keys: `agent`, `harness`, `model`, `trim`, `max_turns`. A
/// value of `unrecorded` matches a model or turn cap the grade does not record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsPick {
    pub agent: Option<Agent>,
    pub harness: Option<u32>,
    pub model: Option<String>,
    pub trim: Option<u32>,
    pub max_turns: Option<String>,
}

impl SettingsPick {
    #[must_use]
    pub fn matches(&self, s: &Settings) -> bool {
        self.agent.is_none_or(|a| a == s.agent)
            && self.harness.is_none_or(|h| h == s.harness)
            && self.model.as_ref().is_none_or(|m| *m == s.model_str())
            && self.trim.is_none_or(|t| t == s.trim)
            && self
                .max_turns
                .as_ref()
                .is_none_or(|m| *m == s.max_turns_str())
    }
}

impl FromStr for SettingsPick {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let mut pick = Self::default();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| format!("`{part}` is not KEY=VALUE"))?;
            let (key, value) = (key.trim(), value.trim());
            let number = |v: &str| {
                v.parse::<u32>()
                    .map_err(|_| format!("{key} must be a number, not `{v}`"))
            };
            match key {
                "agent" => pick.agent = Some(value.parse()?),
                "harness" => pick.harness = Some(number(value)?),
                "model" => pick.model = Some(value.to_owned()),
                "trim" => pick.trim = Some(number(value)?),
                "max_turns" | "max-turns" => {
                    if value != UNRECORDED {
                        number(value)?;
                    }
                    pick.max_turns = Some(value.to_owned());
                }
                other => {
                    return Err(format!(
                    "unknown setting `{other}`; expected agent, harness, model, trim, or max_turns"
                ))
                }
            }
        }
        Ok(pick)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Cell {
    pub arm: ArmName,
    pub repeats: usize,
    pub trials: usize,
    /// The arm's context is empty for this case; these are the none trials.
    pub same_as_none: bool,
}

impl Cell {
    fn rate(&self) -> Option<f64> {
        (self.trials > 0).then(|| self.repeats as f64 / self.trials as f64)
    }
}

/// A case-weighted mean with its n.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rate {
    pub mean: Option<f64>,
    pub ci: Option<(f64, f64)>,
    pub cases: usize,
    pub trials: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Paired {
    /// Mean over paired cases of (arm − none).
    pub mean: Option<f64>,
    pub ci: Option<(f64, f64)>,
    pub cases: usize,
    /// The interval excludes 0 and there are at least two paired cases.
    pub measurable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArmSummary {
    pub arm: ArmName,
    pub rate: Rate,
    /// `None` for the `none` arm itself.
    pub vs_none: Option<Paired>,
    /// Cases where this arm's context is empty and the none trials stand in.
    pub same_as_none: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseRow {
    pub case: String,
    pub what: String,
    pub cells: Vec<Cell>,
}

/// `k` of `n`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Count {
    pub k: usize,
    pub n: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OracleIgnored {
    pub case: String,
    pub what: String,
    pub repeats: usize,
    pub trials: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShownSummary {
    pub arm: ArmName,
    /// Cases whose context holds a relevant lesson, of the cases judged.
    pub shown: Count,
    /// The arm's repeat rate over the shown cases.
    pub arm_rate: Rate,
    /// The none arm's repeat rate over the same cases.
    pub none_rate: Rate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StopRule {
    pub none_repeats: usize,
    pub none_trials: usize,
    /// `none` repeats in at least a third of its graded trials.
    pub can_measure: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub data: Data,
    /// The settings of every run reported; `None` without grades.
    pub settings: Option<Settings>,
    pub seed: u64,
    pub resamples: usize,
    pub labeled_cases: usize,
    pub included_cases: usize,
    pub runs_with_verdict: usize,
    /// UNKNOWN or never judged: excluded.
    pub runs_excluded_unknown: usize,
    /// Runs of cases the label excludes.
    pub runs_on_excluded_cases: usize,
    /// Trial 0 runs (screening), left out of every rate.
    pub screening_runs_excluded: usize,
    /// Cases that did not reproduce at screening, of the cases screened.
    pub screened_out: Option<Count>,
    pub stop: StopRule,
    pub arms: Vec<ArmSummary>,
    pub per_case: Vec<CaseRow>,
    /// (case, arm) cells with 2+ graded trials whose trials disagree.
    pub spread: Count,
    /// Runs with both a deterministic and a YES/NO judge verdict that agree.
    pub grader_agreement: Count,
    /// Cases with oracle trials, and those where the oracle repeats anyway.
    pub oracle_cases: usize,
    pub oracle_ignored: Vec<OracleIgnored>,
    /// `None` without a shown file.
    pub shown: Option<Vec<ShownSummary>>,
}

/// The case-weighted rate over `cells`, with its interval.
fn rate_of(cells: &[Cell], seed: u64) -> Rate {
    let rates: Vec<f64> = cells.iter().filter_map(Cell::rate).collect();
    Rate {
        mean: mean(&rates),
        ci: bootstrap_ci(&rates, RESAMPLES, seed),
        cases: rates.len(),
        trials: cells.iter().map(|c| c.trials).sum(),
    }
}

/// Run the analysis.
#[must_use]
pub fn analyze(input: &Input, data: Data, seed: u64) -> Report {
    let what: HashMap<&str, &str> = input
        .cases
        .iter()
        .map(|(id, w)| (id.as_str(), w.as_str()))
        .collect();

    // The last line per run wins: a resumed `grade` appends.
    let mut runs: HashMap<(&str, ArmName, u32), &GradeLine> = HashMap::new();
    for g in &input.grades {
        runs.insert((g.case.as_str(), g.arm, g.trial), g);
    }
    let before = runs.len();
    runs.retain(|(_, _, trial), _| *trial != 0);
    let screening_runs_excluded = before - runs.len();
    let mut raw: HashMap<(&str, ArmName), (usize, usize)> = HashMap::new();
    let (mut with_verdict, mut unknown, mut outside) = (0, 0, 0);
    let mut agreement = Count::default();
    for ((case, arm, _), g) in &runs {
        if !what.contains_key(case) {
            outside += 1;
            continue;
        }
        if let (Some(det), Some(judge)) = (g.repeat_det, g.judge.and_then(Verdict::as_bool)) {
            agreement.n += 1;
            agreement.k += usize::from(det == judge);
        }
        match g.repeat() {
            Some(repeat) => {
                with_verdict += 1;
                let cell = raw.entry((case, *arm)).or_default();
                cell.0 += usize::from(repeat);
                cell.1 += 1;
            }
            None => unknown += 1,
        }
    }

    let arms: Vec<ArmName> = ArmName::ALL
        .into_iter()
        .filter(|arm| raw.keys().any(|(_, a)| a == arm))
        .collect();
    let cell = |case: &str, arm: ArmName| -> Option<Cell> {
        let substitute = arm != ArmName::None && input.empty.contains(&(case.to_owned(), arm));
        let source = if substitute { ArmName::None } else { arm };
        raw.get(&(case, source)).map(|&(repeats, trials)| Cell {
            arm,
            repeats,
            trials,
            same_as_none: substitute,
        })
    };

    let (none_repeats, none_trials) = input
        .cases
        .iter()
        .filter_map(|(id, _)| raw.get(&(id.as_str(), ArmName::None)))
        .fold((0, 0), |(r, t), &(cr, ct)| (r + cr, t + ct));
    let stop = StopRule {
        none_repeats,
        none_trials,
        can_measure: none_trials > 0 && none_repeats as f64 >= MIN_NONE_RATE * none_trials as f64,
    };

    let per_case: Vec<CaseRow> = input
        .cases
        .iter()
        .map(|(id, w)| CaseRow {
            case: id.clone(),
            what: w.clone(),
            cells: arms.iter().filter_map(|&arm| cell(id, arm)).collect(),
        })
        .collect();
    let cells_of = |arm: ArmName| -> Vec<(&str, Cell)> {
        per_case
            .iter()
            .flat_map(|row| {
                row.cells
                    .iter()
                    .filter(move |c| c.arm == arm && c.trials > 0)
                    .map(move |c| (row.case.as_str(), *c))
            })
            .collect()
    };

    let none_cells: HashMap<&str, Cell> = cells_of(ArmName::None).into_iter().collect();
    let arm_summaries: Vec<ArmSummary> = arms
        .iter()
        .map(|&arm| {
            let cells = cells_of(arm);
            let only: Vec<Cell> = cells.iter().map(|(_, c)| *c).collect();
            let vs_none = (arm != ArmName::None).then(|| {
                let diffs: Vec<f64> = cells
                    .iter()
                    .filter_map(|(case, c)| Some(c.rate()? - none_cells.get(case)?.rate()?))
                    .collect();
                let ci = bootstrap_ci(&diffs, RESAMPLES, seed);
                Paired {
                    mean: mean(&diffs),
                    ci,
                    cases: diffs.len(),
                    measurable: diffs.len() >= 2 && ci.is_some_and(|(lo, hi)| lo > 0.0 || hi < 0.0),
                }
            });
            ArmSummary {
                arm,
                rate: rate_of(&only, seed),
                vs_none,
                same_as_none: cells
                    .iter()
                    .filter(|(_, c)| c.same_as_none)
                    .map(|(case, _)| (*case).to_owned())
                    .collect(),
            }
        })
        .collect();

    let spread = per_case
        .iter()
        .flat_map(|row| row.cells.iter())
        .filter(|c| !c.same_as_none && c.trials >= 2)
        .fold(Count::default(), |acc, c| Count {
            k: acc.k + usize::from(c.repeats > 0 && c.repeats < c.trials),
            n: acc.n + 1,
        });

    let oracle = cells_of(ArmName::Oracle);
    let oracle_ignored = oracle
        .iter()
        .filter(|(_, c)| c.repeats >= 2 && 3 * c.repeats >= 2 * c.trials)
        .map(|(case, c)| OracleIgnored {
            case: (*case).to_owned(),
            what: what.get(case).copied().unwrap_or("").to_owned(),
            repeats: c.repeats,
            trials: c.trials,
        })
        .collect();
    let oracle_cases = oracle.len();
    drop(oracle);

    let shown = (!input.shown.is_empty()).then(|| {
        let mut verdicts: HashMap<(&str, ArmName), Verdict> = HashMap::new();
        for s in &input.shown {
            if what.contains_key(s.case.as_str()) {
                verdicts.insert((s.case.as_str(), s.arm), s.shown);
            }
        }
        [ArmName::Lore, ArmName::File]
            .into_iter()
            .map(|arm| {
                let judged: Vec<&str> = input
                    .cases
                    .iter()
                    .map(|(id, _)| id.as_str())
                    .filter(|id| verdicts.contains_key(&(*id, arm)))
                    .collect();
                let yes: Vec<&str> = judged
                    .iter()
                    .copied()
                    .filter(|id| verdicts.get(&(*id, arm)) == Some(&Verdict::Yes))
                    .collect();
                let pick = |a: ArmName| -> Vec<Cell> {
                    yes.iter()
                        .filter_map(|id| cell(id, a))
                        .filter(|c| c.trials > 0)
                        .collect()
                };
                ShownSummary {
                    arm,
                    shown: Count {
                        k: yes.len(),
                        n: judged.len(),
                    },
                    arm_rate: rate_of(&pick(arm), seed),
                    none_rate: rate_of(&pick(ArmName::None), seed),
                }
            })
            .collect()
    });

    let mut report = Report {
        data,
        settings: input.settings.clone(),
        seed,
        resamples: RESAMPLES,
        labeled_cases: input.labeled,
        included_cases: input.cases.len(),
        runs_with_verdict: with_verdict,
        runs_excluded_unknown: unknown,
        runs_on_excluded_cases: outside,
        screening_runs_excluded,
        screened_out: input.screened_out,
        stop,
        arms: arm_summaries,
        per_case,
        spread,
        grader_agreement: agreement,
        oracle_cases,
        oracle_ignored,
        shown,
    };
    if !report.stop.can_measure {
        report.redact();
    }
    report
}

impl Report {
    /// The stop rule fired: drop every rate, keep the counts.
    fn redact(&mut self) {
        let clear = |r: &mut Rate| {
            r.mean = None;
            r.ci = None;
        };
        for arm in &mut self.arms {
            clear(&mut arm.rate);
            if let Some(p) = &mut arm.vs_none {
                p.mean = None;
                p.ci = None;
                p.measurable = false;
            }
        }
        for s in self.shown.iter_mut().flatten() {
            clear(&mut s.arm_rate);
            clear(&mut s.none_rate);
        }
    }

    /// The report as text, every number tagged with its data source.
    #[must_use]
    pub fn text(&self) -> String {
        let tag = self.data.tag();
        let go = self.stop.can_measure;
        let pct = |x: f64| format!("{:.0}%", x * 100.0);
        let share = |c: Count| {
            if go && c.n > 0 {
                format!("{} of {} ({})", c.k, c.n, pct(c.k as f64 / c.n as f64))
            } else {
                format!("{} of {}", c.k, c.n)
            }
        };
        let rate = |r: &Rate| match (r.mean, r.ci) {
            (Some(m), Some((lo, hi))) => format!(
                "{} [{}, {}] (n={} cases, {} trials)",
                pct(m),
                pct(lo),
                pct(hi),
                r.cases,
                r.trials
            ),
            _ => format!("no rate (n={} cases, {} trials)", r.cases, r.trials),
        };
        let mut s = String::new();
        let settings = self
            .settings
            .as_ref()
            .map(|set| format!(" Settings: {set}."))
            .unwrap_or_default();
        let _ = writeln!(
            s,
            "lore-replay report.{settings} Every number below is measured on {tag}."
        );
        let stop = &self.stop;
        if go {
            let _ = writeln!(
                s,
                "Stop rule: none repeats the mistake in {} of {} graded trials ({}), at least one third, so the replay can measure this archive ({tag}).",
                stop.none_repeats,
                stop.none_trials,
                pct(stop.none_repeats as f64 / stop.none_trials as f64)
            );
        } else {
            let _ = writeln!(
                s,
                "STOP: none repeats the mistake in {} of {} graded trials, fewer than one third. The replay cannot measure this archive; no percentages are printed ({tag}).",
                stop.none_repeats, stop.none_trials
            );
        }
        if go {
            let _ = writeln!(
                s,
                "Intervals: 95%, cluster bootstrap over cases, {} resamples, seed {} ({tag}).",
                self.resamples, self.seed
            );
        }
        if let Some(c) = self.screened_out {
            let _ = writeln!(
                s,
                "screened out: {} of {} cases did not reproduce in the screening run ({tag}).",
                c.k, c.n
            );
        }
        if self.screened_out.is_some() || self.screening_runs_excluded > 0 {
            let _ = writeln!(
                s,
                "Rates are conditional on cases that reproduced at screening; the {} screening runs (trial 0) are left out of every rate ({tag}).",
                self.screening_runs_excluded
            );
        }
        let _ = writeln!(
            s,
            "\nCases: {} of {} labeled cases included (label says mistake and visible) ({tag}).",
            self.included_cases, self.labeled_cases
        );
        let _ = writeln!(
            s,
            "Runs: {} with a verdict (deterministic when present, else judge); {} UNKNOWN or unjudged, excluded; {} on excluded cases, ignored ({tag}).",
            self.runs_with_verdict, self.runs_excluded_unknown, self.runs_on_excluded_cases
        );

        let _ = writeln!(
            s,
            "\nRepeat rate per arm, case-weighted mean over cases ({tag}):"
        );
        for arm in &self.arms {
            let name = arm.arm.as_str();
            let n = format!("n={} cases, {} trials", arm.rate.cases, arm.rate.trials);
            let line = match (&arm.vs_none, go) {
                (_, false) => format!("{n}; no rate (stop rule)"),
                (None, true) => rate(&arm.rate),
                (Some(p), true) if !p.measurable => format!(
                    "no measurable difference from none (paired over n={} cases; {n})",
                    p.cases
                ),
                (Some(p), true) => match (p.mean, p.ci) {
                    (Some(m), Some((lo, hi))) => format!(
                        "{}; vs none {:+.0} points [{:+.0}, {:+.0}] over n={} paired cases",
                        rate(&arm.rate),
                        m * 100.0,
                        lo * 100.0,
                        hi * 100.0,
                        p.cases
                    ),
                    _ => format!("no measurable difference from none ({n})"),
                },
            };
            let _ = writeln!(s, "  {name:<7} {line} ({tag})");
            if !arm.same_as_none.is_empty() {
                let _ = writeln!(
                    s,
                    "          same as none for {} case(s) whose {name} context is empty; their none trials are used: {} ({tag})",
                    arm.same_as_none.len(),
                    arm.same_as_none.join(", ")
                );
            }
        }

        let _ = writeln!(
            s,
            "\nPer case, repeats of graded trials per arm; \"=none\" marks an empty context reported as same as none ({tag}):"
        );
        let idle = self.per_case.iter().filter(|r| r.cells.is_empty()).count();
        for row in self.per_case.iter().filter(|r| !r.cells.is_empty()) {
            let cells: Vec<String> = row
                .cells
                .iter()
                .map(|c| {
                    let mark = if c.same_as_none { "=none" } else { "" };
                    format!("{} {}/{}{mark}", c.arm, c.repeats, c.trials)
                })
                .collect();
            let what = crate::claude::head(&row.what, 70);
            let _ = writeln!(s, "  {}  {}  ({what}) ({tag})", row.case, cells.join("  "));
        }
        if idle > 0 {
            let _ = writeln!(
                s,
                "  {idle} more included case(s) have no graded trials in any arm ({tag})."
            );
        }

        let _ = writeln!(
            s,
            "\nSpread: trials disagree in {} (case, arm) cells with 2+ graded trials ({tag}).",
            share(self.spread)
        );
        let _ = writeln!(
            s,
            "Grader agreement: deterministic and judge agree on {} runs graded by both ({tag}).",
            share(self.grader_agreement)
        );
        let _ = writeln!(
            s,
            "Oracle validity: lesson ignored even when given (oracle repeats in at least 2 of 3 trials) in {} of {} cases with oracle trials ({tag}).",
            self.oracle_ignored.len(),
            self.oracle_cases
        );
        for o in &self.oracle_ignored {
            let _ = writeln!(
                s,
                "  {} oracle {}/{} ({}) ({tag})",
                o.case, o.repeats, o.trials, o.what
            );
        }
        match &self.shown {
            None => {
                let _ = writeln!(s, "Exists/shown/followed: not measured (no --shown file).");
            }
            Some(rows) => {
                let _ = writeln!(s, "Exists/shown/followed ({tag}):");
                for r in rows {
                    let rates = if go {
                        format!(
                            "; among them {} repeats {} vs none {}",
                            r.arm,
                            rate(&r.arm_rate),
                            rate(&r.none_rate)
                        )
                    } else {
                        String::new()
                    };
                    let _ = writeln!(
                        s,
                        "  {}: context holds a relevant lesson for {} judged cases{rates} ({tag})",
                        r.arm,
                        share(r.shown)
                    );
                }
            }
        }
        s
    }
}

#[derive(Debug, Clone)]
pub struct ReportOptions {
    pub labels: PathBuf,
    pub grades: PathBuf,
    pub shown: Option<PathBuf>,
    pub arms: Option<PathBuf>,
    /// `sweep`'s `screen.jsonl`.
    pub screen: Option<PathBuf>,
    /// Report only grades whose settings match (`--settings`, `--harness`).
    pub settings: SettingsPick,
    pub data: Data,
    pub seed: u64,
}

/// Keep the grades whose settings `pick` matches, and the one settings group
/// they share (`None` when none is left). More than one group left is an
/// error naming what differs.
pub fn one_group(
    grades: Vec<GradeLine>,
    pick: &SettingsPick,
) -> Result<(Vec<GradeLine>, Option<Settings>)> {
    let kept: Vec<GradeLine> = grades
        .into_iter()
        .filter(|g| pick.matches(&Settings::of(g)))
        .collect();
    let groups: std::collections::BTreeSet<Settings> = kept.iter().map(Settings::of).collect();
    if groups.len() > 1 {
        let mut parts = Vec::new();
        let mut differ = |what: &str, values: Vec<String>| {
            let mut values = values;
            values.sort();
            values.dedup();
            if values.len() > 1 {
                parts.push(format!("{what} {}", values.join(" and ")));
            }
        };
        differ(
            "agents",
            groups.iter().map(|s| s.agent.to_string()).collect(),
        );
        differ(
            "harness versions",
            groups.iter().map(|s| s.harness.to_string()).collect(),
        );
        differ("models", groups.iter().map(Settings::model_str).collect());
        differ("trims", groups.iter().map(|s| s.trim.to_string()).collect());
        differ(
            "turn caps",
            groups.iter().map(Settings::max_turns_str).collect(),
        );
        return Err(Error::MixedSettings(parts.join("; ")));
    }
    let settings = groups.into_iter().next();
    Ok((kept, settings))
}

/// Cases screened out (`k`) of cases screened (`n`), the last verdict per
/// case winning.
#[must_use]
pub fn screen_count(lines: &[ScreenLine]) -> Count {
    let mut last: HashMap<&str, Verdict> = HashMap::new();
    for line in lines {
        if let Some(v) = line.verdict {
            last.insert(line.case.as_str(), v);
        }
    }
    Count {
        k: last.values().filter(|v| **v != Verdict::Yes).count(),
        n: last.len(),
    }
}

/// Read the files the report needs.
pub fn load(opts: &ReportOptions) -> Result<Input> {
    let labels = read_labels(&opts.labels)?;
    let cases: Vec<(String, String)> = labels
        .iter()
        .filter_map(|c| Some((c.id.clone(), c.included()?.what.clone())))
        .collect();
    let mut empty = HashSet::new();
    if let Some(path) = &opts.arms {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io("read", path, e))?;
        let file: ArmsFile = serde_json::from_str(&text).map_err(|source| Error::Json {
            path: path.clone(),
            source,
        })?;
        for (case, arms) in &file {
            for (arm, text) in arms {
                if let (Ok(arm), true) = (arm.parse::<ArmName>(), text.trim().is_empty()) {
                    empty.insert((case.clone(), arm));
                }
            }
        }
    }
    let read_required = |path: &PathBuf| -> Result<()> {
        std::fs::metadata(path)
            .map(|_| ())
            .map_err(|e| Error::io("read", path, e))
    };
    read_required(&opts.grades)?;
    let shown = match &opts.shown {
        Some(path) => {
            read_required(path)?;
            read_lines(path)?
        }
        None => Vec::new(),
    };
    let screened_out = match &opts.screen {
        Some(path) => {
            read_required(path)?;
            Some(screen_count(&read_lines(path)?))
        }
        None => None,
    };
    let (grades, settings) = one_group(read_lines(&opts.grades)?, &opts.settings)?;
    Ok(Input {
        labeled: labels.iter().filter(|c| c.label.is_some()).count(),
        cases,
        grades,
        shown,
        empty,
        screened_out,
        settings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grade(case: &str, arm: ArmName, trial: u32, verdict: Option<bool>) -> GradeLine {
        GradeLine {
            case: case.into(),
            agent: Agent::ClaudeCode,
            arm,
            trial,
            grader: Some("judge".into()),
            repeat_det: None,
            judge: Some(match verdict {
                Some(true) => Verdict::Yes,
                Some(false) => Verdict::No,
                None => Verdict::Unknown,
            }),
            judge_reason: None,
            fidelity: None,
            foreign_paths: Vec::new(),
            cost_usd: None,
            graded_by: None,
            harness: None,
            model: None,
            trim: None,
            max_turns: None,
        }
    }

    /// `n` cases; per case, `none` repeats in `none` of 3 trials and `arm`
    /// in `arm` of 3.
    fn input(per_case: &[(usize, usize)], arm: ArmName) -> Input {
        let mut grades = Vec::new();
        let mut cases = Vec::new();
        for (i, &(n, a)) in per_case.iter().enumerate() {
            let id = format!("c:{i}");
            for t in 1..=3 {
                grades.push(grade(&id, ArmName::None, t, Some(t <= n as u32)));
                grades.push(grade(&id, arm, t, Some(t <= a as u32)));
            }
            cases.push((id, "w".to_owned()));
        }
        Input {
            labeled: cases.len(),
            cases,
            grades,
            ..Input::default()
        }
    }

    fn arm(r: &Report, name: ArmName) -> &ArmSummary {
        r.arms.iter().find(|a| a.arm == name).unwrap()
    }

    #[test]
    fn the_bootstrap_is_deterministic_for_a_seed() {
        let v = [0.0, 1.0, 0.5, 0.33, 0.67, 1.0, 0.0];
        assert_eq!(bootstrap_ci(&v, 2000, 7), bootstrap_ci(&v, 2000, 7));
        assert_ne!(bootstrap_ci(&v, 2000, 7), bootstrap_ci(&v, 2000, 8));
        let (lo, hi) = bootstrap_ci(&v, 2000, 7).unwrap();
        assert!(lo < mean(&v).unwrap() && mean(&v).unwrap() < hi);
        assert_eq!(bootstrap_ci(&[], 100, 1), None);
    }

    #[test]
    fn a_clear_effect_excludes_zero_and_no_effect_includes_it() {
        let clear = analyze(
            &input(
                &[(3, 0), (3, 1), (2, 0), (3, 0), (3, 1), (2, 0)],
                ArmName::Oracle,
            ),
            Data::Synthetic,
            DEFAULT_SEED,
        );
        let p = arm(&clear, ArmName::Oracle).vs_none.clone().unwrap();
        assert!(p.measurable);
        assert!(p.ci.unwrap().1 < 0.0);
        assert!(clear.text().contains("vs none -78 points"));

        let flat = analyze(
            &input(
                &[(3, 2), (1, 2), (2, 2), (2, 3), (3, 2), (2, 1)],
                ArmName::Oracle,
            ),
            Data::Synthetic,
            DEFAULT_SEED,
        );
        let p = arm(&flat, ArmName::Oracle).vs_none.clone().unwrap();
        let (lo, hi) = p.ci.unwrap();
        assert!(lo <= 0.0 && 0.0 <= hi && !p.measurable);
        assert!(flat
            .text()
            .contains("oracle  no measurable difference from none (paired over n=6 cases"));
    }

    #[test]
    fn the_stop_rule_prints_no_percentages() {
        // none repeats in 2 of 9 trials: under a third.
        let r = analyze(
            &input(&[(1, 0), (1, 0), (0, 0)], ArmName::Lore),
            Data::Real,
            DEFAULT_SEED,
        );
        assert!(!r.stop.can_measure);
        assert_eq!((r.stop.none_repeats, r.stop.none_trials), (2, 9));
        let text = r.text();
        assert!(text
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("STOP: none repeats"));
        assert!(!text.contains('%'), "{text}");
        assert!(arm(&r, ArmName::None).rate.mean.is_none());
        // Exactly a third passes.
        let r = analyze(
            &input(&[(1, 0), (1, 0), (1, 0)], ArmName::Lore),
            Data::Real,
            1,
        );
        assert!(r.stop.can_measure);
    }

    #[test]
    fn an_empty_arm_is_the_none_arm() {
        let mut inp = input(&[(3, 0), (2, 0), (3, 1)], ArmName::Lore);
        inp.empty.insert(("c:1".into(), ArmName::Lore));
        let r = analyze(&inp, Data::Synthetic, DEFAULT_SEED);
        let lore = arm(&r, ArmName::Lore);
        assert_eq!(lore.same_as_none, ["c:1"]);
        let row = &r.per_case[1];
        let cell = row.cells.iter().find(|c| c.arm == ArmName::Lore).unwrap();
        assert_eq!((cell.repeats, cell.trials, cell.same_as_none), (2, 3, true));
        // The substituted cell pairs to a difference of 0 and is not spread.
        assert_eq!(lore.vs_none.as_ref().unwrap().cases, 3);
        assert_eq!(r.spread.n, 5);
        assert!(r.text().contains("lore 2/3=none"));
    }

    #[test]
    fn unknown_verdicts_are_excluded_and_counted() {
        let mut inp = input(&[(3, 0), (3, 0)], ArmName::Oracle);
        inp.grades.push(grade("c:0", ArmName::Oracle, 7, None));
        inp.grades.push(grade("c:9", ArmName::None, 1, Some(true)));
        let mut det_only = grade("c:1", ArmName::Oracle, 8, None);
        det_only.judge = None;
        inp.grades.push(det_only);
        // A later line for the same run replaces the earlier one.
        inp.grades.push(grade("c:0", ArmName::None, 1, None));
        let r = analyze(&inp, Data::Synthetic, DEFAULT_SEED);
        assert_eq!(r.runs_excluded_unknown, 3);
        assert_eq!(r.runs_on_excluded_cases, 1);
        assert_eq!(r.runs_with_verdict, 11);
        assert_eq!(r.per_case[0].cells[0].trials, 2);
    }

    #[test]
    fn deterministic_verdicts_win_and_agreement_is_counted() {
        let mut inp = input(&[(3, 0)], ArmName::Oracle);
        inp.grades[0].repeat_det = Some(false); // judge YES, det NO
        inp.grades[2].repeat_det = Some(true); // judge YES, det YES
        let r = analyze(&inp, Data::Synthetic, DEFAULT_SEED);
        assert_eq!(r.grader_agreement, Count { k: 1, n: 2 });
        assert_eq!(r.per_case[0].cells[0].repeats, 2);
    }
}
