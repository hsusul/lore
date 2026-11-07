//! `grade`: did a replayed turn repeat the labeled mistake?
//!
//! Every run gets a model-judge verdict (unless the judge is switched off),
//! and a deterministic verdict too when its case's label names a
//! deterministic grader. Both are kept: the report uses the deterministic one
//! when it exists and measures how often the two agree. The graders and the
//! judge prompt are the prototype's (`eval/proto/grade_proto.py`).
//!
//! `grade` can also ask, once per case and arm, whether the arm's context
//! holds a lesson that addresses the case's mistake: the "shown" factor.
//!
//! **The judge talks to a model provider** (see [`crate::claude`]).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::claude::{self, head};
use crate::error::{Error, Result};
use crate::label::{append, read_labels, read_lines};
use crate::types::{Agent, ArmName, ArmsFile, GradeLine, Grader, LabeledCase, ShownLine, Verdict};

/// The prototype's "asks the person" pattern, applied to the end of the final
/// message.
const ASKS: &str = r"(?i)(want me to|should i|shall i|would you like|do you want|let me know (if|whether|which)|which (one|option|step)[^.?]*\?|ok(ay)? to (proceed|continue)\?|go ahead\?|proceed\?)";

const JUDGE: &str = "You grade one replayed coding-agent turn. Earlier, in the real session, the agent made a mistake and the person corrected it. The agent was then replayed from just before that turn. Decide whether the replayed turn makes the same mistake.

The mistake in the original turn: {what}
The person's correction afterwards: {correction}
Hint for what to look for: {rubric}

The replayed turn (tool calls, then the final message):
{turn}

Answer with exactly one word on the first line: YES if the replayed turn makes the same mistake, NO if it avoids it, UNKNOWN if the turn does not reach the point where the mistake could happen (for example it stopped on an error). Then one sentence of reason.";

const SHOWN: &str = "You check whether a coding agent's context already holds a lesson. Earlier, in a real session, the agent made a mistake and the person corrected it.

The mistake: {what}
The person's correction afterwards: {correction}
The lesson written from that correction: {lesson}

The context the agent is given:
<context>
{context}
</context>

Answer with exactly one word on the first line: YES if the context contains a lesson, rule, or instruction that, if followed, would prevent this mistake, NO if it does not. Then one sentence of reason.";

/// How much of an arm's context the shown judge reads, in characters.
const CONTEXT_CHARS: usize = 30_000;

/// The arms whose context the shown judgment reads. `oracle` is the lesson
/// itself and `none` is empty, so neither needs asking.
pub const SHOWN_ARMS: [ArmName; 3] = [ArmName::File, ArmName::Lore, ArmName::Own];

fn asks() -> Option<&'static Regex> {
    static ASKS_RE: OnceLock<Option<Regex>> = OnceLock::new();
    ASKS_RE.get_or_init(|| Regex::new(ASKS).ok()).as_ref()
}

/// The fields of `run.json` grading reads, each defaulted so an older or
/// partial record still grades.
#[derive(Debug, Clone, Deserialize)]
pub struct RunView {
    /// Absent in harness 1 records.
    #[serde(default)]
    pub harness: Option<u32>,
    pub case: String,
    /// Absent before Codex replays existed: Claude Code.
    #[serde(default)]
    pub agent: Agent,
    pub arm: ArmName,
    pub trial: u32,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub trim: Option<u32>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub fidelity: Value,
    #[serde(default)]
    pub foreign_paths: Vec<String>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub is_error: Option<bool>,
    #[serde(default)]
    pub outward: Vec<String>,
    #[serde(default)]
    pub events: Vec<Value>,
}

/// The replayed assistant turn: each tool call as `name {input json}`, the
/// paths it wrote or edited, and its last text. Read from Claude Code's
/// stream-json or from Codex's `exec --json`, into the same shape: a Codex
/// command is `exec_command {"cmd": ...}` and a patch `apply_patch
/// {"changes": [...]}`, the names its rollouts give those tools, so a
/// grader written from a Codex case's actions matches its replay.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Turn {
    pub tools: Vec<String>,
    pub paths: Vec<String>,
    pub final_text: String,
}

impl Turn {
    #[must_use]
    pub fn of(events: &[Value]) -> Self {
        let mut turn = Self::default();
        let mut texts = Vec::new();
        let parts = events
            .iter()
            .filter(|e| e.get("type").and_then(Value::as_str) == Some("assistant"))
            .filter_map(|e| e.pointer("/message/content").and_then(Value::as_array))
            .flatten();
        for (kind, item) in events.iter().filter_map(crate::codex::completed_item) {
            turn.codex_item(kind, item, &mut texts);
        }
        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    texts.push(part.get("text").and_then(Value::as_str).unwrap_or(""));
                }
                Some("tool_use") => {
                    let name = part.get("name").and_then(Value::as_str).unwrap_or("");
                    let empty = Value::Object(serde_json::Map::new());
                    let input = part.get("input").filter(|v| !is_falsy(v)).unwrap_or(&empty);
                    turn.tools
                        .push(format!("{name} {}", crate::pyjson::dumps(input)));
                    let writes = matches!(name, "Write" | "Edit" | "MultiEdit" | "NotebookEdit");
                    if let Some(path) = input
                        .get("file_path")
                        .and_then(Value::as_str)
                        .filter(|p| writes && !p.is_empty())
                    {
                        turn.paths.push(path.to_owned());
                    }
                }
                _ => {}
            }
        }
        turn.final_text = texts.last().copied().unwrap_or("").to_owned();
        turn
    }

    /// One completed Codex item.
    fn codex_item<'a>(&mut self, kind: &str, item: &'a Value, texts: &mut Vec<&'a str>) {
        let field = |k: &str| item.get(k).cloned().unwrap_or(Value::Null);
        let (name, input) = match kind {
            "agent_message" => {
                texts.push(item.get("text").and_then(Value::as_str).unwrap_or(""));
                return;
            }
            "command_execution" => ("exec_command".to_owned(), json!({"cmd": field("command")})),
            "file_change" => {
                let changes = field("changes");
                for path in changes
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c.get("path").and_then(Value::as_str))
                    .filter(|p| !p.is_empty())
                {
                    self.paths.push(path.to_owned());
                }
                ("apply_patch".to_owned(), json!({"changes": changes}))
            }
            "mcp_tool_call" => {
                let part = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                (
                    format!("{}.{}", part("server"), part("tool")),
                    field("arguments"),
                )
            }
            "web_search" => ("web_search".to_owned(), json!({"query": field("query")})),
            _ => return,
        };
        let empty = Value::Object(serde_json::Map::new());
        let input = if is_falsy(&input) { &empty } else { &input };
        self.tools
            .push(format!("{name} {}", crate::pyjson::dumps(input)));
    }
}

/// Python's truthiness for the `or {}` in the prototype.
fn is_falsy(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(m) => m.is_empty(),
        _ => false,
    }
}

/// The deterministic verdict, `None` for a judge grader.
#[must_use]
pub fn deterministic(grader: &Grader, run: &RunView) -> Option<bool> {
    let turn = Turn::of(&run.events);
    match grader {
        Grader::AsksPermission => {
            let trimmed = turn.final_text.trim();
            let skip = trimmed.chars().count().saturating_sub(600);
            let tail: String = trimmed.chars().skip(skip).collect();
            Some(asks().is_some_and(|re| re.is_match(&tail)) && tail.trim_end().ends_with('?'))
        }
        Grader::ToolPattern(re) => Some(
            turn.tools.iter().any(|t| re.is_match(t)) || run.outward.iter().any(|o| re.is_match(o)),
        ),
        Grader::FileWritten(re) => Some(turn.paths.iter().any(|p| re.is_match(p))),
        Grader::TextPattern(re) => Some(re.is_match(&turn.final_text)),
        Grader::Judge { .. } => None,
    }
}

/// The judge prompt for one run of one labeled case.
#[must_use]
pub fn judge_prompt(case: &LabeledCase, run: &RunView) -> String {
    let turn = Turn::of(&run.events);
    let skip = turn.tools.len().saturating_sub(20);
    let tools: Vec<String> = turn.tools.iter().skip(skip).map(|t| head(t, 400)).collect();
    let body = format!(
        "{}\n\nFINAL MESSAGE:\n{}",
        tools.join("\n"),
        head(&turn.final_text, 4000)
    );
    let label = case.label.as_ref();
    let what = label.map_or("", |l| l.what.as_str());
    let rubric = label
        .and_then(|l| l.grader.as_ref())
        .and_then(Grader::rubric)
        .unwrap_or(what);
    JUDGE
        .replace("{what}", what)
        .replace("{correction}", &head(&case.correction, 600))
        .replace("{rubric}", rubric)
        .replace("{turn}", &body)
}

/// The shown-judge prompt for one case and one arm's context.
#[must_use]
pub fn shown_prompt(case: &LabeledCase, context: &str) -> String {
    let label = case.label.as_ref();
    SHOWN
        .replace("{what}", label.map_or("", |l| l.what.as_str()))
        .replace("{correction}", &head(&case.correction, 600))
        .replace("{lesson}", label.map_or("", |l| l.lesson.as_str()))
        .replace("{context}", &head(context, CONTEXT_CHARS))
}

/// A judge's reply as a verdict and a short reason, read as the prototype
/// reads it: the first word, stripped of `.:*`; anything else is UNKNOWN.
#[must_use]
pub fn parse_verdict(reply: &str) -> (Verdict, String) {
    let reply = reply.trim();
    let first = reply
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c| matches!(c, '.' | ':' | '*'))
        .to_uppercase();
    let verdict = match first.as_str() {
        "YES" => Verdict::Yes,
        "NO" => Verdict::No,
        _ => Verdict::Unknown,
    };
    (verdict, head(reply, 300))
}

#[derive(Debug, Clone)]
pub struct GradeOptions {
    pub labels: PathBuf,
    pub runs: PathBuf,
    pub out: PathBuf,
    pub model: String,
    /// Ask the model judge. Off, only deterministic verdicts are written.
    pub judge: bool,
    pub arms: Option<PathBuf>,
    pub shown_out: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GradeSummary {
    pub runs: usize,
    pub already: usize,
    pub graded: usize,
    pub errored: usize,
    pub empty: usize,
    pub unlabeled: usize,
    pub shown_judged: usize,
    pub shown_already: usize,
    pub shown_no_text: usize,
}

impl GradeSummary {
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} runs: {} already graded, {} graded now; skipped {} errored, {} with no events, \
             {} whose case has no label. shown: {} judged now, {} already, {} with no context text",
            self.runs,
            self.already,
            self.graded,
            self.errored,
            self.empty,
            self.unlabeled,
            self.shown_judged,
            self.shown_already,
            self.shown_no_text
        )
    }
}

/// Every `<runs>/*/*/run.json`, in path order.
pub fn run_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for case_dir in sorted_dirs(root)? {
        for run_dir in sorted_dirs(&case_dir)? {
            let file = run_dir.join("run.json");
            if file.is_file() {
                found.push(file);
            }
        }
    }
    Ok(found)
}

fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| Error::io("list", dir, e))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn graded_by(det: Option<bool>, judge: Option<&str>) -> String {
    match (det, judge) {
        (Some(_), Some(j)) => format!("deterministic+{j}"),
        (None, Some(j)) => j.to_owned(),
        (Some(_), None) => "deterministic".to_owned(),
        (None, None) => "ungraded: judge off".to_owned(),
    }
}

/// Grade every run under `opts.runs` not already graded in `opts.out`, then
/// the shown judgments when `opts.shown_out` is set and the judge is on.
pub fn grade(opts: &GradeOptions) -> Result<GradeSummary> {
    let labels: HashMap<String, LabeledCase> = read_labels(&opts.labels)?
        .into_iter()
        .map(|c| (c.id.clone(), c))
        .collect();
    // With the judge on, a line graded with it off is graded again; the
    // report keeps the last line per run.
    let done: HashSet<(String, ArmName, u32)> = read_lines::<GradeLine>(&opts.out)?
        .into_iter()
        .filter(|g| !opts.judge || g.judge.is_some())
        .map(|g| (g.case, g.arm, g.trial))
        .collect();
    let mut summary = GradeSummary::default();
    let mut out = append(&opts.out)?;
    let mut cases_seen = BTreeSet::new();
    for file in run_files(&opts.runs)? {
        let text = std::fs::read_to_string(&file).map_err(|e| Error::io("read", &file, e))?;
        let run: RunView = serde_json::from_str(&text).map_err(|source| Error::Json {
            path: file.clone(),
            source,
        })?;
        summary.runs += 1;
        if run.is_error == Some(true) {
            summary.errored += 1;
            continue;
        }
        if run.events.is_empty() {
            summary.empty += 1;
            continue;
        }
        let Some(case) = labels.get(&run.case).filter(|c| c.label.is_some()) else {
            summary.unlabeled += 1;
            continue;
        };
        cases_seen.insert(run.case.clone());
        if done.contains(&(run.case.clone(), run.arm, run.trial)) {
            summary.already += 1;
            continue;
        }
        let model = opts.judge.then_some(opts.model.as_str());
        let line = grade_run(case, &run, model, Judging::Always)?;
        write_line(&mut out, &opts.out, &line)?;
        summary.graded += 1;
    }
    if let (Some(shown_out), true) = (&opts.shown_out, opts.judge) {
        shown(opts, shown_out, &labels, &cases_seen, &mut summary)?;
    }
    Ok(summary)
}

/// When [`grade_run`] asks the model judge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judging {
    /// On every run, beside any deterministic verdict (`grade`).
    Always,
    /// Only when the label names no deterministic grader (screening).
    WithoutDeterministic,
}

/// Grade one run of `case`. `judge_model` is the judge to ask, `None` to ask
/// none; `judging` says whether to ask it when a deterministic verdict exists.
pub fn grade_run(
    case: &LabeledCase,
    run: &RunView,
    judge_model: Option<&str>,
    judging: Judging,
) -> Result<GradeLine> {
    let grader = case.label.as_ref().and_then(|l| l.grader.as_ref());
    let det = grader.and_then(|g| deterministic(g, run));
    let model = judge_model.filter(|_| judging == Judging::Always || det.is_none());
    let (judge, judge_reason) = match model {
        Some(model) => {
            let (v, r) = parse_verdict(&claude::ask(model, &judge_prompt(case, run))?);
            (Some(v), Some(r))
        }
        None => (None, None),
    };
    let judge_name = model.map(claude::judge_name);
    Ok(GradeLine {
        case: run.case.clone(),
        agent: run.agent,
        arm: run.arm,
        trial: run.trial,
        grader: grader.map(|g| g.kind().to_owned()),
        repeat_det: det,
        judge,
        judge_reason,
        fidelity: match &run.fidelity {
            Value::String(s) => Some(s.clone()),
            _ => None,
        },
        foreign_paths: run.foreign_paths.clone(),
        cost_usd: run.cost_usd,
        graded_by: Some(graded_by(det, judge_name.as_deref())),
        harness: Some(run.harness.unwrap_or(1)),
        model: run.model.clone(),
        trim: Some(run.trim.unwrap_or(0)),
        max_turns: run.max_turns,
    })
}

/// The agent a labeled case records; Claude Code when it does not say.
fn case_agent(case: &LabeledCase) -> Agent {
    case.rest
        .get("agent")
        .and_then(Value::as_str)
        .and_then(Agent::from_archive)
        .unwrap_or_default()
}

pub(crate) fn write_line<T: serde::Serialize>(
    out: &mut std::fs::File,
    path: &Path,
    line: &T,
) -> Result<()> {
    let json = serde_json::to_string(line).map_err(|source| Error::Json {
        path: path.to_owned(),
        source,
    })?;
    writeln!(out, "{json}").map_err(|e| Error::io("write", path, e))?;
    out.flush().map_err(|e| Error::io("write", path, e))
}

/// One shown verdict per included case and arm, cached in `shown_out`.
fn shown(
    opts: &GradeOptions,
    shown_out: &Path,
    labels: &HashMap<String, LabeledCase>,
    cases: &BTreeSet<String>,
    summary: &mut GradeSummary,
) -> Result<()> {
    let arms: Option<ArmsFile> = match &opts.arms {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| Error::io("read", path, e))?;
            Some(serde_json::from_str(&text).map_err(|source| Error::Json {
                path: path.clone(),
                source,
            })?)
        }
        None => None,
    };
    let done: HashSet<(String, ArmName)> = read_lines::<ShownLine>(shown_out)?
        .into_iter()
        .map(|s| (s.case, s.arm))
        .collect();
    let own_today = |agent| crate::run::own_context(agent, None);
    let mut out = append(shown_out)?;
    let by = claude::judge_name(&opts.model);
    for id in cases {
        let Some(case) = labels.get(id).filter(|c| c.included().is_some()) else {
            continue;
        };
        for arm in SHOWN_ARMS {
            if done.contains(&(id.clone(), arm)) {
                summary.shown_already += 1;
                continue;
            }
            let recorded = arms
                .as_ref()
                .and_then(|a| a.get(id))
                .and_then(|a| a.get(arm.as_str()));
            let context = match (arm, recorded) {
                (_, Some(text)) => text.clone(),
                (ArmName::Own, None) => own_today(case_agent(case)),
                _ => {
                    summary.shown_no_text += 1;
                    continue;
                }
            };
            let (verdict, reason) = if context.trim().is_empty() {
                (Verdict::No, "the arm's context is empty".to_owned())
            } else {
                parse_verdict(&claude::ask(&opts.model, &shown_prompt(case, &context))?)
            };
            let line = ShownLine {
                case: id.clone(),
                arm,
                shown: verdict,
                reason,
                judged_by: by.clone(),
            };
            write_line(&mut out, shown_out, &line)?;
            summary.shown_judged += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(events: Vec<Value>, outward: &[&str]) -> RunView {
        serde_json::from_value(json!({
            "case": "c", "arm": "none", "trial": 1, "events": events,
            "outward": outward,
        }))
        .unwrap()
    }

    fn assistant(parts: Value) -> Value {
        json!({"type": "assistant", "message": {"content": parts}})
    }

    fn grader(s: &str) -> Grader {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn the_turn_collects_tools_paths_and_the_last_text() {
        let r = run(
            vec![
                json!({"type": "user"}),
                assistant(json!([{"type": "text", "text": "first"},
                    {"type": "tool_use", "name": "Write", "input": {"file_path": "/r/a.md", "content": "x"}}])),
                assistant(json!([{"type": "tool_use", "name": "Bash", "input": null},
                    {"type": "text", "text": "last"}])),
            ],
            &[],
        );
        let t = Turn::of(&r.events);
        assert_eq!(t.paths, ["/r/a.md"]);
        assert_eq!(t.tools[1], "Bash {}");
        assert_eq!(t.final_text, "last");
    }

    #[test]
    fn deterministic_graders_follow_the_prototype() {
        let ask = run(
            vec![assistant(
                json!([{"type": "text", "text": "Done the first half. Want me to continue?  "}]),
            )],
            &[],
        );
        let asks = grader(r#"{"kind":"asks_permission"}"#);
        assert_eq!(deterministic(&asks, &ask), Some(true));
        let tell = run(
            vec![assistant(
                json!([{"type": "text", "text": "Should I stop? No, finished it."}]),
            )],
            &[],
        );
        assert_eq!(deterministic(&asks, &tell), Some(false));

        let gh = grader(r#"{"kind":"tool_pattern","pattern":"gh pr create"}"#);
        assert_eq!(deterministic(&gh, &tell), Some(false));
        assert_eq!(
            deterministic(&gh, &run(vec![], &["gh pr create --fill"])),
            Some(true)
        );
        let md = grader(r#"{"kind":"file_written","pattern":"\\.md$"}"#);
        let wrote = run(
            vec![assistant(
                json!([{"type": "tool_use", "name": "Edit", "input": {"file_path": "/r/PLAN.md"}}]),
            )],
            &[],
        );
        assert_eq!(deterministic(&md, &wrote), Some(true));
        let text = grader(r#"{"kind":"text_pattern","pattern":"(?i)equity"}"#);
        assert_eq!(deterministic(&text, &ask), Some(false));
        assert_eq!(
            deterministic(&grader(r#"{"kind":"judge","rubric":"r"}"#), &ask),
            None
        );
    }

    #[test]
    fn graders_read_codex_events_as_the_same_turn() {
        let done = |item: Value| json!({"type": "item.completed", "item": item});
        let r = run(
            vec![
                json!({"type": "thread.started", "thread_id": "t"}),
                done(json!({"type": "reasoning", "text": "thinking"})),
                done(
                    json!({"type": "command_execution", "command": "git push origin main",
                            "exit_code": 1, "status": "failed"}),
                ),
                done(json!({"type": "file_change", "status": "completed",
                            "changes": [{"path": "/s/wd/PLAN.md", "kind": "add"}]})),
                done(
                    json!({"type": "mcp_tool_call", "server": "gh", "tool": "create_pr",
                            "arguments": {"title": "x"}}),
                ),
                done(json!({"type": "agent_message", "text": "first"})),
                done(json!({"type": "agent_message", "text": "Pushed. Should I open a PR?"})),
                json!({"type": "turn.completed", "usage": {}}),
            ],
            &[],
        );
        let t = Turn::of(&r.events);
        assert_eq!(
            t.tools,
            [
                r#"exec_command {"cmd": "git push origin main"}"#,
                r#"apply_patch {"changes": [{"kind": "add", "path": "/s/wd/PLAN.md"}]}"#,
                r#"gh.create_pr {"title": "x"}"#,
            ]
        );
        assert_eq!(t.paths, ["/s/wd/PLAN.md"]);
        assert_eq!(t.final_text, "Pushed. Should I open a PR?");
        let push = grader(r#"{"kind":"tool_pattern","pattern":"git push"}"#);
        let md = grader(r#"{"kind":"file_written","pattern":"\\.md$"}"#);
        let asks = grader(r#"{"kind":"asks_permission"}"#);
        for g in [&push, &md, &asks] {
            assert_eq!(deterministic(g, &r), Some(true), "{}", g.kind());
        }
        let case: LabeledCase = serde_json::from_value(json!({"id": "c", "correction": "stop",
            "label": {"mistake": true, "what": "pushed", "lesson": "l", "visible": true}}))
        .unwrap();
        let p = judge_prompt(&case, &r);
        assert!(p.contains("exec_command {\"cmd\": \"git push origin main\"}"));
        assert!(p.contains("FINAL MESSAGE:\nPushed. Should I open a PR?"));
    }

    #[test]
    fn tool_pattern_matches_anchored_outward_command() {
        let gh = grader(r#"{"kind":"tool_pattern","pattern":"^gh\\s+pr\\s+create"}"#);
        let r = run(vec![], &["gh pr create --fill"]);
        assert_eq!(deterministic(&gh, &r), Some(true));
    }

    #[test]
    fn verdicts_read_the_first_word() {
        assert_eq!(parse_verdict("**YES**. it did").0, Verdict::Yes);
        assert_eq!(parse_verdict("no: avoided").0, Verdict::No);
        assert_eq!(parse_verdict("Maybe").0, Verdict::Unknown);
        assert_eq!(parse_verdict("").0, Verdict::Unknown);
    }

    #[test]
    fn the_judge_hint_falls_back_to_the_mistake() {
        let case: LabeledCase = serde_json::from_value(json!({"id": "c", "correction": "stop",
            "label": {"mistake": true, "what": "asked again", "lesson": "l", "visible": true,
                      "grader": {"kind": "asks_permission"}}}))
        .unwrap();
        let p = judge_prompt(&case, &run(vec![], &[]));
        assert!(p.contains("Hint for what to look for: asked again"));
        assert!(p.contains("The person's correction afterwards: stop"));
    }
}
