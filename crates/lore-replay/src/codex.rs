//! Codex CLI sessions: reading a rollout, building a replay prefix from one,
//! and the `codex exec` command a replay runs.
//!
//! A rollout is `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`,
//! one `{timestamp, type, payload}` object per line. As with Claude Code, a
//! line number is a replay coordinate, so the reader keeps every line.
//!
//! Isolation differs from Claude Code's in how, not in what:
//! - The replay runs with its own `CODEX_HOME` in the run directory. Codex
//!   writes the resumed session there, and Lore never reads that folder, so
//!   nothing reaches the archive. It also keeps the person's `AGENTS.md`,
//!   memories, skills, and plugins out of every arm, as the Claude Code path
//!   keeps CLAUDE.md and auto memory out. Only `auth.json` is shared, as a
//!   symlink, so the run uses the person's login (see [`link_auth`]).
//! - Shell commands run under a Codex permission profile ([`profile`])
//!   instead of `-s workspace-write`: the legacy sandbox reads everywhere and
//!   writes `/tmp`, and a profile can hide `/Users` and `/Volumes` the way the
//!   Claude Code sandbox does. Codex ignores profiles when `-s` is passed, so
//!   the argv has none.
//! - Codex has no turn cap; [`Cap`] stops the agent after `--max-turns`
//!   tool calls, which is where Claude Code's cap lands.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::prefix::{tool_output_omitted, Prefix, Rewrite, IMAGE_OMITTED};
use crate::session;
use crate::types::Action;

/// Overrides the `codex` found on `PATH`.
pub const CODEX_ENV: &str = "LORE_REPLAY_CODEX";
/// The permission profile name the replay defines and selects.
pub const PROFILE: &str = "lore-replay";
/// What [`crate::run::requested_model`] records when no `--model` is given.
pub const DEFAULT_MODEL: &str = "codex-default";
/// The injected user message that carries `AGENTS.md`.
const AGENTS_MD: &str = "# AGENTS.md instructions";

/// The person's Codex folder: `$CODEX_HOME`, or `~/.codex`.
#[must_use]
pub fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
}

fn payload(entry: &Value) -> Option<&Value> {
    entry.get("payload")
}

fn kind(entry: &Value) -> Option<&str> {
    entry.get("type").and_then(Value::as_str)
}

fn payload_kind(entry: &Value) -> Option<&str> {
    payload(entry)
        .and_then(|p| p.get("type"))
        .and_then(Value::as_str)
}

fn is_message(entry: &Value, role: &str) -> bool {
    kind(entry) == Some("response_item")
        && payload_kind(entry) == Some("message")
        && payload(entry)
            .and_then(|p| p.get("role"))
            .and_then(Value::as_str)
            == Some(role)
}

/// The text a message line carries: its `input_text` / `output_text` parts
/// joined by newlines.
#[must_use]
pub fn content_text(entry: &Value) -> String {
    payload(entry)
        .and_then(|p| p.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("input_text" | "output_text" | "text")
            )
        })
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// True for a turn a person typed: a user message that is not context Codex
/// injected (`<environment_context>`, `AGENTS.md`, and the like). The miner
/// skips the same turns.
#[must_use]
pub fn is_human(entry: &Value) -> bool {
    if !is_message(entry, "user") {
        return false;
    }
    let text = content_text(entry);
    let text = text.trim_start();
    !text.is_empty() && !text.starts_with('<') && !text.starts_with(AGENTS_MD)
}

/// What the assistant did across `entries`: non-blank prose and tool calls,
/// in order. A `function_call`'s JSON arguments become the input; a custom
/// tool's raw input (an `apply_patch` body, say) is `{"input": ...}`.
#[must_use]
pub fn assistant_actions(entries: &[Value]) -> Vec<Action> {
    let mut out = Vec::new();
    for entry in entries {
        if kind(entry) != Some("response_item") {
            continue;
        }
        let Some(p) = payload(entry) else { continue };
        let name = || {
            p.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        match payload_kind(entry) {
            Some("message") if is_message(entry, "assistant") => {
                let text = content_text(entry);
                if !text.trim().is_empty() {
                    out.push(Action::Text { text });
                }
            }
            Some("function_call") => {
                let input = match p.get("arguments") {
                    Some(Value::String(s)) => {
                        serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.clone()))
                    }
                    Some(other) => other.clone(),
                    None => Value::Null,
                };
                out.push(Action::Tool {
                    name: name(),
                    input,
                });
            }
            Some("custom_tool_call") => out.push(Action::Tool {
                name: name(),
                input: json!({"input": p.get("input").cloned().unwrap_or(Value::Null)}),
            }),
            Some("local_shell_call") => out.push(Action::Tool {
                name: "shell".into(),
                input: p.get("action").cloned().unwrap_or(Value::Null),
            }),
            _ => {}
        }
    }
    out
}

/// The working directory in force at line `at`: the last `session_meta` or
/// `turn_context` `cwd` on or before it.
#[must_use]
pub fn cwd_at(entries: &[Value], at: usize) -> Option<String> {
    entries
        .get(..=at.min(entries.len().saturating_sub(1)))
        .unwrap_or_default()
        .iter()
        .rev()
        .filter(|e| matches!(kind(e), Some("session_meta" | "turn_context")))
        .find_map(|e| {
            payload(e)
                .and_then(|p| p.get("cwd"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
}

/// The branch `session_meta` recorded, when it recorded git.
#[must_use]
pub fn branch(entries: &[Value]) -> Option<String> {
    entries
        .iter()
        .filter(|e| kind(e) == Some("session_meta"))
        .find_map(|e| {
            payload(e)?
                .pointer("/git/branch")?
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
}

/// Whether `session_meta` says the rollout is a sub-agent's, one a parent
/// thread spawned (`source: {"subagent": ...}`, `thread_source: "subagent"`).
/// Codex refuses to resume a multi-agent v2 sub-agent on its own ("resume the
/// parent first"), so such a rollout cannot be replayed.
#[must_use]
pub fn is_subagent(entries: &[Value]) -> bool {
    entries
        .iter()
        .filter(|e| kind(e) == Some("session_meta"))
        .filter_map(payload)
        .any(|p| {
            p.pointer("/source/subagent").is_some()
                || p.get("thread_source").and_then(Value::as_str) == Some("subagent")
        })
}

/// Every rollout under `home`'s `sessions` and `archived_sessions`, by the
/// session id at the end of its name.
#[must_use]
pub fn rollout_index(home: &Path) -> HashMap<String, PathBuf> {
    let mut found = Vec::new();
    collect(&home.join("sessions"), 3, &mut found);
    collect(&home.join("archived_sessions"), 0, &mut found);
    found.sort();
    let mut index = HashMap::new();
    for path in found {
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .and_then(|stem| {
                stem.len()
                    .checked_sub(36)
                    .and_then(|at| stem.get(at..))
                    .map(str::to_owned)
            });
        if let Some(id) = id {
            index.entry(id).or_insert(path);
        }
    }
    index
}

/// Rollout files up to `depth` real directories below `dir`; symlinks are
/// not followed, as Lore's discovery does not follow them.
fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() && depth > 0 {
            collect(&entry.path(), depth - 1, out);
        } else if file_type.is_file() && name.starts_with("rollout-") && name.ends_with(".jsonl") {
            out.push(entry.path());
        }
    }
}

/// The first `prompt_line` lines of a rollout, moved into the scratch clone:
/// a fresh session id in `session_meta`, every `cwd` and mention of the
/// original repository rewritten, and the person's `AGENTS.md` (the injected
/// user message and the `world_state` snapshot that repeats it) left out, as
/// every arm runs without it. `has_messages` is true when there is a
/// `session_meta` line, which `codex exec resume` needs.
#[must_use]
pub fn prefix(bytes: &[u8], prompt_line: usize, rewrite: &Rewrite<'_>) -> Prefix {
    let scratch = rewrite.scratch.to_string_lossy();
    let replacements = rewrite.replacements();
    let entries: Vec<Value> = session::split_lines(bytes)
        .take(prompt_line)
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter(|e| kind(e) != Some("world_state"))
        .filter(|e| !(is_message(e, "user") && content_text(e).starts_with(AGENTS_MD)))
        .collect();
    let items = entries
        .iter()
        .filter(|e| kind(e) == Some("response_item"))
        .count();
    let trim_before = if rewrite.trim == 0 {
        0
    } else {
        items.saturating_sub(rewrite.trim)
    };
    let mut out = Prefix {
        text: String::new(),
        has_messages: false,
    };
    let mut item = 0;
    for mut entry in entries {
        match kind(&entry) {
            Some("session_meta") => {
                out.has_messages = true;
                if let Some(p) = entry.get_mut("payload").and_then(Value::as_object_mut) {
                    p.insert("id".into(), rewrite.session_id.into());
                    if p.contains_key("session_id") {
                        p.insert("session_id".into(), rewrite.session_id.into());
                    }
                }
            }
            Some("response_item") => {
                if item < trim_before {
                    trim_item(&mut entry);
                }
                item += 1;
            }
            _ => {}
        }
        if let Some(p) = entry.get_mut("payload").and_then(Value::as_object_mut) {
            if p.contains_key("cwd") {
                p.insert("cwd".into(), scratch.as_ref().into());
            }
        }
        rewrite.apply(&mut entry, &replacements);
        out.text.push_str(&entry.to_string());
        out.text.push('\n');
    }
    out
}

/// Trim one `response_item`: a tool output becomes [`tool_output_omitted`]
/// with its length, and each image part becomes [`IMAGE_OMITTED`] text.
fn trim_item(entry: &mut Value) {
    let Some(p) = entry.get_mut("payload") else {
        return;
    };
    if let Some(output) = p.get_mut("output") {
        let chars = match &*output {
            Value::String(s) => s.chars().count(),
            Value::Null => 0,
            other => other.to_string().chars().count(),
        };
        if chars > 0 {
            *output = Value::String(tool_output_omitted(chars));
        }
    }
    if let Some(parts) = p.get_mut("content").and_then(Value::as_array_mut) {
        for part in parts {
            if part.get("type").and_then(Value::as_str) == Some("input_image") {
                *part = json!({"type": "input_text", "text": IMAGE_OMITTED});
            }
        }
    }
}

/// Where Codex looks for session `id`: under `home`'s `sessions`, in the
/// date folder of `at_ms`.
#[must_use]
pub fn rollout_path(home: &Path, id: &str, at_ms: i64) -> PathBuf {
    let at = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(at_ms) * 1_000_000)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    let (y, m, d) = (at.year(), u8::from(at.month()), at.day());
    home.join("sessions")
        .join(format!("{y:04}"))
        .join(format!("{m:02}"))
        .join(format!("{d:02}"))
        .join(format!(
            "rollout-{y:04}-{m:02}-{d:02}T{:02}-{:02}-{:02}-{id}.jsonl",
            at.hour(),
            at.minute(),
            at.second()
        ))
}

/// Paths a replay's shell commands may use beyond the read-only system.
#[derive(Debug, Clone)]
pub struct Reach<'a> {
    /// The stub tools and git config (read only).
    pub tools: &'a Path,
    /// The commands' temp folder (`TMPDIR`), inside the scratch.
    pub tmp: &'a Path,
    /// The file the `gh` stub appends to.
    pub outward_log: &'a Path,
}

/// The permission profile, as the TOML inline table `-c` takes: read the
/// system, hide `/Users` and `/Volumes`, write only the workspace and the
/// scratch temp folder, read the stub tools, append the outward log, and no
/// network.
#[must_use]
pub fn profile(reach: &Reach<'_>) -> String {
    let q = |p: &Path| toml_string(&p.to_string_lossy());
    format!(
        "{{filesystem={{\":root\"=\"read\",\"/Users\"=\"deny\",\"/Volumes\"=\"deny\",\
         \":workspace_roots\"={{\".\"=\"write\"}},{}=\"read\",{}=\"write\",{}=\"write\"}},\
         network={{enabled=false}}}}",
        q(reach.tools),
        q(reach.tmp),
        q(reach.outward_log),
    )
}

/// `s` as a TOML basic string. JSON's escapes are a subset of TOML's.
fn toml_string(s: &str) -> String {
    Value::String(s.to_owned()).to_string()
}

/// The `codex exec` argv. Like Claude Code's, it depends only on the case and
/// the options a sweep shares, never on the arm or trial. The prompt comes on
/// stdin (`-`). `-C` is an `exec` option: `exec resume` rejects it.
#[must_use]
pub fn args(resume: Option<&str>, model: Option<&str>, wd: &Path, profile: &str) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "exec".into(),
        "-C".into(),
        wd.to_string_lossy().into_owned(),
    ];
    if let Some(id) = resume {
        args.extend(["resume".into(), id.into()]);
    }
    args.extend([
        "-".into(),
        "--json".into(),
        "--skip-git-repo-check".into(),
        "--ignore-user-config".into(),
        "-c".into(),
        format!("default_permissions={}", toml_string(PROFILE)),
        "-c".into(),
        format!("permissions.{PROFILE}={profile}"),
        "-c".into(),
        "approval_policy=\"never\"".into(),
        "-c".into(),
        "project_doc_max_bytes=0".into(),
    ]);
    if let Some(model) = model {
        args.extend(["-m".into(), model.into()]);
    }
    args
}

/// Link `home/auth.json` to the person's, so the replay runs under their
/// login without sharing anything else of their Codex folder.
pub fn link_auth(home: &Path, real: &Path) -> Result<()> {
    let link = home.join("auth.json");
    let target = real.join("auth.json");
    if target.is_file() {
        crate::run::symlink(&target, &link).map_err(|e| Error::io("link", &link, e))?;
    }
    Ok(())
}

/// If Codex refreshed the login by replacing the link with a file, copy the
/// new credentials back, so the person's own login is not left holding a
/// refresh token the refresh used up.
pub fn restore_auth(home: &Path, real: &Path) {
    let link = home.join("auth.json");
    let replaced = std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_file());
    if replaced {
        let _ = std::fs::copy(&link, real.join("auth.json"));
    }
}

/// The model the replayed turn ran: the last `turn_context` model in any
/// rollout under `home`.
#[must_use]
pub fn model_used(home: &Path) -> Option<String> {
    let mut files = Vec::new();
    collect(&home.join("sessions"), 3, &mut files);
    files.sort();
    files.iter().rev().find_map(|file| {
        let entries = session::read_lines(file).ok()?;
        entries
            .iter()
            .rev()
            .filter(|e| kind(e) == Some("turn_context"))
            .find_map(|e| payload(e)?.get("model")?.as_str().map(str::to_owned))
    })
}

/// An `exec --json` item that is a tool call: what the cap counts.
const TOOL_ITEMS: [&str; 4] = [
    "command_execution",
    "file_change",
    "mcp_tool_call",
    "web_search",
];

/// The `item` of an `item.completed` event, and its type.
#[must_use]
pub fn completed_item(event: &Value) -> Option<(&str, &Value)> {
    if event.get("type").and_then(Value::as_str) != Some("item.completed") {
        return None;
    }
    let item = event.get("item")?;
    Some((item.get("type").and_then(Value::as_str)?, item))
}

/// Counts tool calls and says when the run has reached `--max-turns`.
#[derive(Debug, Clone)]
pub struct Cap {
    max: u32,
    tools: u32,
    finished: bool,
}

impl Cap {
    #[must_use]
    pub fn new(max: u32) -> Self {
        Self {
            max,
            tools: 0,
            finished: false,
        }
    }

    /// Note `event`; true when the agent should be stopped now.
    pub fn reached(&mut self, event: &Value) -> bool {
        match event.get("type").and_then(Value::as_str) {
            Some("turn.completed" | "turn.failed") => self.finished = true,
            _ => {
                if completed_item(event).is_some_and(|(t, _)| TOOL_ITEMS.contains(&t)) {
                    self.tools += 1;
                }
            }
        }
        !self.finished && self.max > 0 && self.tools >= self.max
    }
}

/// The run's outcome from its events: `is_error` (a failed turn, or no
/// finished turn without a cap) and the error text.
#[must_use]
pub fn outcome(events: &[Value], capped: bool) -> (bool, Option<String>) {
    let failed = events
        .iter()
        .find(|e| e.get("type").and_then(Value::as_str) == Some("turn.failed"));
    let message = failed
        .and_then(|e| e.pointer("/error/message"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let completed = events
        .iter()
        .any(|e| e.get("type").and_then(Value::as_str) == Some("turn.completed"));
    (failed.is_some() || (!completed && !capped), message)
}

/// Every error message among the events.
pub fn error_messages(events: &[Value]) -> impl Iterator<Item = &str> {
    events
        .iter()
        .filter_map(|e| match e.get("type").and_then(Value::as_str) {
            Some("turn.failed") => e.pointer("/error/message").and_then(Value::as_str),
            Some("error") => e.get("message").and_then(Value::as_str),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(payload: Value) -> Value {
        json!({"timestamp": "2026-01-01T00:00:00Z", "type": "response_item", "payload": payload})
    }

    fn user(text: &str) -> Value {
        item(json!({"type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": text}]}))
    }

    #[test]
    fn injected_context_is_not_a_human_turn() {
        assert!(is_human(&user("fix the bug")));
        assert!(!is_human(&user(
            "<environment_context>x</environment_context>"
        )));
        assert!(!is_human(&user("# AGENTS.md instructions for /r\n...")));
        let developer = item(json!({"type": "message", "role": "developer",
                                    "content": [{"type": "input_text", "text": "x"}]}));
        assert!(!is_human(&developer));
        let echo =
            json!({"type": "event_msg", "payload": {"type": "user_message", "message": "x"}});
        assert!(!is_human(&echo));
    }

    #[test]
    fn actions_read_function_and_custom_tool_calls() {
        let entries = [
            item(json!({"type": "message", "role": "assistant",
                        "content": [{"type": "output_text", "text": "on it"}]})),
            item(json!({"type": "function_call", "name": "exec_command",
                        "arguments": "{\"cmd\":\"git push\"}", "call_id": "c1"})),
            item(json!({"type": "custom_tool_call", "name": "apply_patch",
                        "input": "*** Begin Patch", "call_id": "c2"})),
            item(json!({"type": "function_call_output", "call_id": "c1", "output": "ok"})),
        ];
        assert_eq!(
            assistant_actions(&entries),
            vec![
                Action::Text {
                    text: "on it".into()
                },
                Action::Tool {
                    name: "exec_command".into(),
                    input: json!({"cmd": "git push"})
                },
                Action::Tool {
                    name: "apply_patch".into(),
                    input: json!({"input": "*** Begin Patch"})
                },
            ]
        );
    }

    #[test]
    fn the_prefix_takes_a_new_id_the_scratch_path_and_no_agents_md() {
        let lines = [
            json!({"type": "session_meta", "payload": {"id": "old", "cwd": "/r",
                    "git": {"branch": "main"}}}),
            user("# AGENTS.md instructions\nbe nice"),
            json!({"type": "world_state", "payload": {"agents_md": {"text": "be nice"}}}),
            json!({"type": "turn_context", "payload": {"cwd": "/r", "model": "m"}}),
            item(json!({"type": "function_call_output", "call_id": "c", "output": "long output"})),
            user("look at /r/a.rs"),
            user("the prompt"),
        ];
        let bytes: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let scratch = PathBuf::from("/s/wd");
        let rewrite = Rewrite {
            session_id: "new",
            scratch: &scratch,
            toplevel: Some(Path::new("/r")),
            home: None,
            trim: 1,
        };
        let built = prefix(bytes.as_bytes(), 6, &rewrite);
        assert!(built.has_messages);
        let got: Vec<Value> = built
            .text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            got.len(),
            4,
            "AGENTS.md, world_state, and the prompt are left out"
        );
        assert_eq!(got[0]["payload"]["id"], "new");
        assert_eq!(got[0]["payload"]["cwd"], "/s/wd");
        assert_eq!(got[1]["payload"]["cwd"], "/s/wd");
        assert_eq!(
            got[2]["payload"]["output"],
            tool_output_omitted(11),
            "older output is trimmed"
        );
        assert_eq!(
            content_text(&got[3]),
            "look at /s/wd/a.rs",
            "the last item is kept"
        );
        let whole = prefix(bytes.as_bytes(), 6, &Rewrite { trim: 0, ..rewrite });
        assert!(whole.text.contains("long output"));
        assert_eq!(branch(&lines), Some("main".into()));
        assert_eq!(cwd_at(&lines, 5), Some("/r".into()));
    }

    #[test]
    fn rollouts_are_named_by_date_and_id() {
        let path = rollout_path(Path::new("/h"), "abc", 1_767_323_045_678);
        assert_eq!(
            path,
            Path::new("/h/sessions/2026/01/02/rollout-2026-01-02T03-04-05-abc.jsonl")
        );
    }

    #[test]
    fn the_cap_stops_after_that_many_tool_calls_unless_the_turn_ended() {
        let done = |t: &str| json!({"type": "item.completed", "item": {"type": t}});
        let mut cap = Cap::new(2);
        assert!(!cap.reached(&done("agent_message")));
        assert!(!cap.reached(&done("command_execution")));
        assert!(cap.reached(&done("file_change")));
        let mut ended = Cap::new(1);
        assert!(!ended.reached(&json!({"type": "turn.completed"})));
        assert!(!ended.reached(&done("command_execution")));
    }

    #[test]
    fn the_profile_hides_home_folders_and_the_network() {
        let p = profile(&Reach {
            tools: Path::new("/v/s/home"),
            tmp: Path::new("/v/s/home/tmp"),
            outward_log: Path::new("/v/r/outward.log"),
        });
        assert_eq!(
            p,
            "{filesystem={\":root\"=\"read\",\"/Users\"=\"deny\",\"/Volumes\"=\"deny\",\
             \":workspace_roots\"={\".\"=\"write\"},\"/v/s/home\"=\"read\",\
             \"/v/s/home/tmp\"=\"write\",\"/v/r/outward.log\"=\"write\"},network={enabled=false}}"
        );
    }

    #[test]
    fn a_run_without_a_finished_turn_is_an_error_unless_capped() {
        let failed = [json!({"type": "turn.failed", "error": {"message": "limit"}})];
        assert_eq!(outcome(&failed, false), (true, Some("limit".into())));
        assert_eq!(outcome(&[], false), (true, None));
        assert_eq!(outcome(&[], true), (false, None));
        assert_eq!(
            outcome(&[json!({"type": "turn.completed"})], false),
            (false, None)
        );
    }
}
