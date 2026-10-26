//! How each agent CLI is launched headless, and how its output is summarized.
//!
//! Launch flags were verified against `claude --help` (2.1.270) and
//! `codex exec --help` (0.154.0) on 2026-09-14. Lore never passes a flag that
//! bypasses the agent's permission prompts or sandbox (ADR-0007).

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

use lore_ipc::{ActivityDto, ActivityKind, ProviderKind, TaskAgent, TaskEffort, TaskPermission};

use crate::providers::ProviderLaunch;

/// Program used to launch each agent. Defaults to `claude` / `codex` on `PATH`;
/// tests substitute a fake agent script.
#[derive(Debug, Clone)]
pub struct AgentPrograms {
    pub claude_code: PathBuf,
    pub codex: PathBuf,
}

impl Default for AgentPrograms {
    fn default() -> Self {
        Self {
            claude_code: PathBuf::from("claude"),
            codex: PathBuf::from("codex"),
        }
    }
}

/// How to start an agent run.
#[derive(Debug, Clone)]
pub struct Launch<'a> {
    pub agent: TaskAgent,
    pub permission: TaskPermission,
    pub worktree: &'a Path,
    pub prompt: &'a str,
    /// Resume this agent session instead of starting a new one (Claude only).
    pub resume_session: Option<&'a str>,
    /// Claude `--model` / Codex `--model`. None uses the agent's default.
    pub model: Option<&'a str>,
    /// Claude `--effort` / Codex `model_reasoning_effort`.
    pub effort: Option<TaskEffort>,
    /// A model provider for this run (Codex only).
    pub provider: Option<&'a ProviderLaunch>,
}

/// Environment variable a custom provider's API key reaches Codex through, so
/// the key never appears in the agent's arguments.
pub const PROVIDER_KEY_ENV: &str = "LORE_PROVIDER_API_KEY";

/// A model id Lore will pass as a CLI argument: letters, digits, `.`, `_`,
/// `-`, `:` and `/` (for ids like `gpt-oss:20b` and `qwen/qwen3-coder`), not
/// starting with `-`, `.` or `/` and without `..`, so it cannot be read as a
/// flag or a path.
pub fn parse_model(raw: Option<&str>) -> crate::Result<Option<String>> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if raw.len() > 80
        || raw.starts_with(['-', '.', '/'])
        || raw.contains("..")
        || !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'))
    {
        return Err(crate::Error::Invalid(format!(
            "model must be a short id such as opus, claude-opus-5-5, gpt-5.6-sol, or gpt-oss:20b (got {raw})"
        )));
    }
    Ok(Some(raw.to_string()))
}

/// Build the headless command for one agent run.
pub fn command(programs: &AgentPrograms, launch: Launch<'_>) -> Command {
    let mut cmd = match launch.agent {
        TaskAgent::ClaudeCode => {
            let mut cmd = Command::new(&programs.claude_code);
            // acceptEdits lets the agent edit files in its worktree without
            // auto-approving shell commands; `auto` delegates routine approvals to
            // Claude's own classifier. Never bypassPermissions. stream-json
            // requires --verbose. Print mode skips the workspace-trust dialog, so
            // settings and MCP servers committed to the repository (which may be
            // untrusted) are not loaded: only the user's own and local settings.
            let mode = match launch.permission {
                TaskPermission::Edits => "acceptEdits",
                TaskPermission::Auto => "auto",
            };
            cmd.args([
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                mode,
                "--setting-sources",
                "user,local",
                "--strict-mcp-config",
            ]);
            if let Some(session) = launch.resume_session {
                cmd.arg(format!("--resume={session}"));
            }
            if let Some(model) = launch.model {
                cmd.arg("--model").arg(model);
            }
            if let Some(effort) = launch.effort {
                cmd.arg("--effort").arg(effort.as_claude());
            }
            cmd.arg("--").arg(launch.prompt);
            cmd
        }
        TaskAgent::Codex => {
            let mut cmd = Command::new(&programs.codex);
            // workspace-write confines writes to the worktree via Codex's sandbox.
            // `codex exec resume` (0.144.6) accepts neither --sandbox nor --cd, so
            // Codex continuations start a new session with a brief instead.
            cmd.args(["exec", "--json", "--sandbox", "workspace-write", "--cd"])
                .arg(launch.worktree);
            if let Some(provider) = launch.provider {
                provider_flags(&mut cmd, provider);
            }
            if let Some(model) = launch.model {
                cmd.arg("--model").arg(model);
            }
            if let Some(effort) = launch.effort {
                cmd.arg("-c")
                    .arg(format!("model_reasoning_effort=\"{}\"", effort.as_codex()));
            }
            cmd.arg("--").arg(launch.prompt);
            cmd
        }
    };
    cmd.current_dir(launch.worktree);
    cmd
}

/// Point one Codex run at a provider. Local ones use Codex's own `--oss`
/// support; a custom one is defined for this run only with `-c` overrides, so
/// the user's `~/.codex/config.toml` is never touched.
fn provider_flags(cmd: &mut Command, provider: &ProviderLaunch) {
    match provider.kind {
        ProviderKind::Ollama => {
            cmd.args(["--oss", "--local-provider", "ollama"]);
        }
        ProviderKind::Lmstudio => {
            cmd.args(["--oss", "--local-provider", "lmstudio"]);
        }
        ProviderKind::Custom => {
            let key = format!("lore-{}", provider.id);
            let mut set = |field: &str, value: &str| {
                cmd.arg("-c").arg(format!(
                    "model_providers.{key}.{field}={}",
                    toml_string(value)
                ));
            };
            set("name", &provider.name);
            if let Some(url) = &provider.base_url {
                set("base_url", url);
            }
            set("wire_api", "responses");
            if let Some(api_key) = &provider.api_key {
                set("env_key", PROVIDER_KEY_ENV);
                cmd.env(PROVIDER_KEY_ENV, api_key);
            }
            cmd.arg("-c")
                .arg(format!("model_provider={}", toml_string(&key)));
        }
    }
}

/// `value` as a TOML basic string, for `codex -c key=value`.
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const SESSION_TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// Claude's session id from the latest run in the log, if that run was a
/// Claude run that reported one. Only `{"type":"system","subtype":"init"}`
/// events count, and the id must be a UUID so it can never be read as a flag.
pub fn claude_session_id(log: &Path) -> Option<String> {
    let text = read_tail(log, SESSION_TAIL_BYTES)?;
    let latest_run = text
        .rsplit_once(RUN_MARKER)
        .map_or(text.as_str(), |(_, rest)| rest);
    latest_run
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .filter(|v| {
            v.get("type").and_then(|t| t.as_str()) == Some("system")
                && v.get("subtype").and_then(|t| t.as_str()) == Some("init")
        })
        .find_map(|v| {
            v.get("session_id")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .filter(|s| is_uuid(s))
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

fn read_tail(log: &Path, max: u64) -> Option<String> {
    let mut file = File::open(log).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(max))).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some(if len > max {
        text.split_once('\n')
            .map_or(String::new(), |(_, r)| r.to_string())
    } else {
        text
    })
}

/// A problem in the latest run that the user must act on, if any. Only errors
/// count (failed results and stderr lines), never agent prose or tool calls.
pub fn attention(log: &Path) -> Option<String> {
    let events = activity_with_tail(log, 30, 64 * 1024);
    let start = events
        .iter()
        .rposition(|e| e.kind == ActivityKind::Output && e.text.starts_with(RUN_MARKER))
        .map_or(0, |i| i + 1);
    events[start..]
        .iter()
        .rev()
        .filter(|e| matches!(e.kind, ActivityKind::Error | ActivityKind::Output))
        .find_map(|e| {
            if is_claude_hook_failure(&e.text) {
                return None;
            }
            let lower = e.text.to_lowercase();
            // Only quota exhaustion, not transient 429 "rate limit" retries.
            if lower.contains("usage limit")
                || lower.contains("hit your limit")
                || lower.contains("quota exceeded")
            {
                Some(
                    "Usage limit reached. Continue later or hand off to the other agent."
                        .to_string(),
                )
            } else if lower.contains("failed to authenticate")
                || lower.contains("not logged in")
                || lower.contains("please run /login")
                || lower.contains("invalid api key")
            {
                Some(format!(
                    "Agent is not signed in. Run it once in a terminal to log in. ({})",
                    cap_chars(&e.text, 120)
                ))
            } else {
                None
            }
        })
}

/// Whether the latest run stopped because the agent ran out of quota, which is
/// what an automatic handoff to the other agent reacts to.
pub fn usage_limited(log: &Path) -> bool {
    attention(log).is_some_and(|a| a.starts_with("Usage limit"))
}

/// Written between runs in a task log.
pub const RUN_MARKER: &str = "--- Lore:";

fn cap_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

const TAIL_BYTES: u64 = 64 * 1024;
const MAX_ACTIVITY_CHARS: usize = 240;

/// The most recent human-readable line of an agent's log, if any.
///
/// Understands Claude Code `stream-json` and Codex `--json` events loosely: it
/// prefers assistant text / final results, and otherwise falls back to the last
/// non-JSON line (e.g. an error printed to stderr).
pub fn last_activity(log: &Path) -> Option<String> {
    let mut file = File::open(log).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    // A read that starts mid-file almost always starts mid-line; drop that
    // fragment so partial JSON (possibly tool output) is never shown.
    let text = if len > TAIL_BYTES {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };

    // Prefer what the agent said; fall back to a plain line (e.g. a startup
    // error on stderr) only when no event carries text. Trailing stderr noise
    // such as a failing hook must not hide the agent's final result.
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let from_events = lines.iter().rev().find_map(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| event_text(&v))
    });
    from_events
        .or_else(|| {
            lines
                .iter()
                .rev()
                .find(|l| !l.starts_with('{') && !is_claude_hook_failure(l))
                .map(|l| l.to_string())
        })
        .map(|s| truncate(&s))
}

fn event_text(v: &serde_json::Value) -> Option<String> {
    // Claude Code: {"type":"result","result":"..."}
    if let Some(result) = v.get("result").and_then(|r| r.as_str()) {
        return non_empty(result);
    }
    // Claude Code: {"type":"assistant","message":{"content":[{"type":"text","text":"..."}]}}
    if let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) {
        let text = content
            .iter()
            .filter_map(|b| match b.get("type").and_then(|t| t.as_str()) {
                Some("text") => b.get("text").and_then(|t| t.as_str()).map(str::to_string),
                Some("tool_use") => b
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(|n| format!("using {n}")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        return non_empty(&text);
    }
    // Codex errors: {"type":"error","message":...} and turn.failed.
    if matches!(
        v.get("type").and_then(|t| t.as_str()),
        Some("error") | Some("turn.failed")
    ) {
        return ["/message", "/error/message"]
            .iter()
            .find_map(|p| v.pointer(p).and_then(|s| s.as_str()))
            .and_then(non_empty);
    }
    // Codex: only agent messages, never reasoning items.
    if let Some(item) = v.get("item") {
        return match item.get("type").and_then(|t| t.as_str()) {
            Some("agent_message") => item
                .get("text")
                .and_then(|t| t.as_str())
                .and_then(non_empty),
            _ => None,
        };
    }
    for ptr in ["/msg/message", "/message", "/payload/message"] {
        if let Some(s) = v.pointer(ptr).and_then(|m| m.as_str()) {
            return non_empty(s);
        }
    }
    None
}

const ACTIVITY_TAIL_BYTES: u64 = 512 * 1024;
const MAX_EVENT_CHARS: usize = 2_000;

/// Claude settings/hooks can fail on stderr without the agent itself failing.
/// Those lines mention "error" so they used to show as errors and steal focus.
fn is_claude_hook_failure(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("hook")
        && (l.contains("econnrefused")
            || l.contains("hook failed")
            || (l.contains("hook [") && l.contains("failed")))
}

fn line_kind(line: &str) -> ActivityKind {
    if is_claude_hook_failure(line) {
        ActivityKind::Output
    } else if line.to_ascii_lowercase().contains("error") {
        ActivityKind::Error
    } else {
        ActivityKind::Output
    }
}

/// The agent's recent activity as a readable timeline, oldest first.
///
/// Thinking/reasoning and tool results (which may contain whole files) are
/// never included; tool calls are summarized by name and target.
pub fn activity(log: &Path, max_events: usize) -> Vec<ActivityDto> {
    activity_with_tail(log, max_events, ACTIVITY_TAIL_BYTES)
}

fn activity_with_tail(log: &Path, max_events: usize, tail: u64) -> Vec<ActivityDto> {
    let Some(text) = read_tail(log, tail) else {
        return Vec::new();
    };
    let mut events: Vec<ActivityDto> = Vec::new();
    // Codex reports an item when it starts and again when it completes. Keep
    // the first so a running command shows; ids restart with each run.
    let mut seen_items: HashSet<String> = HashSet::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => {
                if v.get("type").and_then(|t| t.as_str()) == Some("thread.started") {
                    seen_items.clear();
                }
                let id = v.pointer("/item/id").and_then(|i| i.as_str());
                if id.is_some_and(|id| seen_items.contains(id)) {
                    continue;
                }
                let rows = activity_from_event(&v);
                if let (Some(id), false) = (id, rows.is_empty()) {
                    seen_items.insert(id.to_string());
                }
                events.extend(rows);
            }
            Err(_) if !line.starts_with('{') => events.push(ActivityDto {
                kind: line_kind(line),
                text: cap(line),
            }),
            Err(_) => {}
        }
    }
    let skip = events.len().saturating_sub(max_events);
    events.split_off(skip)
}

fn activity_from_event(v: &serde_json::Value) -> Vec<ActivityDto> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<ActivityDto>, kind, text: &str| {
        if let Some(text) = non_empty(text) {
            out.push(ActivityDto {
                kind,
                text: cap(&text),
            });
        }
    };
    match v.get("type").and_then(|t| t.as_str()) {
        Some("result") => {
            let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
            let kind = if is_error {
                ActivityKind::Error
            } else {
                ActivityKind::Result
            };
            if let Some(r) = v.get("result").and_then(|r| r.as_str()) {
                push(&mut out, kind, r);
            }
        }
        Some("assistant") => {
            for block in v
                .pointer("/message/content")
                .and_then(|c| c.as_array())
                .into_iter()
                .flatten()
            {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(s) = block.get("text").and_then(|s| s.as_str()) {
                            push(&mut out, ActivityKind::Message, s);
                        }
                    }
                    Some("tool_use") => {
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                        let target = [
                            "file_path",
                            "path",
                            "command",
                            "pattern",
                            "url",
                            "description",
                        ]
                        .iter()
                        .find_map(|k| {
                            block
                                .pointer(&format!("/input/{k}"))
                                .and_then(|s| s.as_str())
                        })
                        .unwrap_or("");
                        push(&mut out, ActivityKind::Tool, &format!("{name} {target}"));
                    }
                    _ => {}
                }
            }
        }
        _ => {
            if let Some(item) = v.get("item") {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("agent_message") => {
                        if let Some(s) = item.get("text").and_then(|s| s.as_str()) {
                            push(&mut out, ActivityKind::Message, s);
                        }
                    }
                    Some("command_execution") => {
                        if let Some(s) = item.get("command").and_then(|s| s.as_str()) {
                            push(&mut out, ActivityKind::Tool, &format!("run {s}"));
                        }
                    }
                    Some("file_change") | Some("patch") => {
                        push(&mut out, ActivityKind::Tool, "edit files");
                    }
                    Some("error") => {
                        if let Some(s) = item.get("message").and_then(|s| s.as_str()) {
                            push(&mut out, ActivityKind::Error, s);
                        }
                    }
                    _ => {}
                }
            } else if let Some(s) = ["/message", "/error/message"]
                .iter()
                .filter(|_| {
                    matches!(
                        v.get("type").and_then(|t| t.as_str()),
                        Some("error") | Some("turn.failed")
                    )
                })
                .find_map(|p| v.pointer(p).and_then(|s| s.as_str()))
            {
                push(&mut out, ActivityKind::Error, s);
            }
        }
    }
    out
}

fn cap(s: &str) -> String {
    if s.chars().count() <= MAX_EVENT_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_EVENT_CHARS - 1).collect();
    out.push('…');
    out
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_ACTIVITY_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_ACTIVITY_CHARS - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write;

    fn log_with(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f
    }

    #[test]
    fn prefers_latest_claude_assistant_text() {
        let f = log_with(&[
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Reading the  parser"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"x"}]}}"#,
        ]);
        assert_eq!(
            last_activity(f.path()).as_deref(),
            Some("Reading the parser")
        );
    }

    #[test]
    fn reports_claude_tool_use_and_result() {
        let f = log_with(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit"}]}}"#,
        ]);
        assert_eq!(last_activity(f.path()).as_deref(), Some("using Edit"));
        let f = log_with(&[r#"{"type":"result","result":"Done: fixed it"}"#]);
        assert_eq!(last_activity(f.path()).as_deref(), Some("Done: fixed it"));
    }

    #[test]
    fn falls_back_to_plain_stderr_line() {
        let f = log_with(&[r#"{"type":"system"}"#, "Error: usage limit reached"]);
        assert_eq!(
            last_activity(f.path()).as_deref(),
            Some("Error: usage limit reached")
        );
    }

    #[test]
    fn final_result_wins_over_trailing_stderr_noise() {
        // Shape observed from a real `claude -p` run whose auth had expired.
        let f = log_with(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Failed to authenticate"}]}}"#,
            r#"{"type":"result","is_error":true,"result":"Failed to authenticate: session expired"}"#,
            "SessionEnd hook [http://127.0.0.1:1/hook] failed: connect ECONNREFUSED",
        ]);
        assert_eq!(
            last_activity(f.path()).as_deref(),
            Some("Failed to authenticate: session expired")
        );
    }

    #[test]
    fn activity_timeline_skips_thinking_and_tool_results() {
        let f = log_with(&[
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"private"},{"type":"text","text":"Looking at auth"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/auth.ts"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"whole file contents"}]}}"#,
            r#"{"type":"result","is_error":false,"result":"Fixed"}"#,
            "hook failed: connect ECONNREFUSED error",
        ]);
        let events = activity(f.path(), 50);
        let got: Vec<_> = events.iter().map(|e| (e.kind, e.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (ActivityKind::Message, "Looking at auth"),
                (ActivityKind::Tool, "Edit src/auth.ts"),
                (ActivityKind::Result, "Fixed"),
                (
                    ActivityKind::Output,
                    "hook failed: connect ECONNREFUSED error"
                ),
            ]
        );
        assert_eq!(activity(f.path(), 2).len(), 2);
        assert!(!events
            .iter()
            .any(|e| e.text.contains("private") || e.text.contains("whole file")));
    }

    #[test]
    fn claude_hook_failures_are_output_and_never_attention() {
        let f = log_with(&[
            "SessionEnd hook [http://127.0.0.1:8737/agents/hooks/claude/event?managed_by=headroom&version=3] failed: connect ECONNREFUSED 127.0.0.1:8737",
        ]);
        let events = activity(f.path(), 10);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ActivityKind::Output);
        assert!(attention(f.path()).is_none());
        assert!(last_activity(f.path()).is_none());
    }

    fn args(launch: Launch<'_>) -> Vec<String> {
        command(&AgentPrograms::default(), launch)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn resume_and_permission_flags() {
        let base = Launch {
            agent: TaskAgent::ClaudeCode,
            permission: TaskPermission::Auto,
            worktree: Path::new("/w"),
            prompt: "-p looks like a flag",
            resume_session: Some("abc"),
            model: None,
            effort: None,
            provider: None,
        };
        let a = args(base.clone());
        let mode = a.iter().position(|x| x == "--permission-mode").unwrap();
        assert_eq!(a[mode + 1], "auto");
        assert!(a.iter().any(|x| x == "--resume=abc"));
        assert_eq!(&a[a.len() - 2..], ["--", "-p looks like a flag"]);
        assert!(!a
            .iter()
            .any(|x| x.contains("dangerously") || x == "bypassPermissions"));

        let c = args(Launch {
            agent: TaskAgent::Codex,
            ..base
        });
        assert!(
            !c.iter().any(|x| x == "resume"),
            "codex never resumes (loses sandbox)"
        );
        assert_eq!(&c[c.len() - 2..], ["--", "-p looks like a flag"]);
    }

    #[test]
    fn finds_latest_session_id_and_attention() {
        let uuid_old = "11111111-2222-3333-4444-555555555555";
        let uuid_new = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let f = log_with(&[
            &format!(r#"{{"type":"system","subtype":"init","session_id":"{uuid_old}"}}"#),
            "--- Lore: run 2 (Claude Code) ---",
            &format!(r#"{{"type":"system","subtype":"init","session_id":"{uuid_new}"}}"#),
            r#"{"type":"result","is_error":true,"result":"Claude AI usage limit reached|1789300000"}"#,
        ]);
        assert_eq!(claude_session_id(f.path()).as_deref(), Some(uuid_new));
        assert!(attention(f.path()).unwrap().contains("Usage limit"));

        // A later non-Claude run, spoofed ids, or flag-like ids never resume.
        let codex_last = log_with(&[
            &format!(r#"{{"type":"system","subtype":"init","session_id":"{uuid_old}"}}"#),
            "--- Lore: run 2 (Codex) ---",
            r#"{"thread_id":"019a-codex"}"#,
        ]);
        assert_eq!(claude_session_id(codex_last.path()), None);
        let spoof = log_with(&[
            r#"{"type":"system","subtype":"init","session_id":"--dangerously-skip-permissions"}"#,
            r#"{"session_id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}"#,
        ]);
        assert_eq!(claude_session_id(spoof.path()), None);

        let ok = log_with(&[
            r#"{"type":"result","is_error":true,"result":"usage limit reached"}"#,
            "--- Lore: run 2 (Claude Code) ---",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"added rate limit middleware"}]}}"#,
            r#"{"type":"result","is_error":false,"result":"Done"}"#,
        ]);
        assert_eq!(
            attention(ok.path()),
            None,
            "earlier runs and prose do not count"
        );
    }

    /// Shapes captured from `codex exec --json` 0.154.0 on 2026-09-13.
    #[test]
    fn codex_usage_limit_is_detected() {
        let f = log_with(&[
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Skill descriptions were shortened to fit the skills context budget."}}"#,
            r#"{"type":"error","message":"You've hit your usage limit. Upgrade to Pro or try again at 4:30 PM."}"#,
            r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit. Upgrade to Pro or try again at 4:30 PM."}}"#,
        ]);
        assert!(attention(f.path()).unwrap().contains("Usage limit reached"));
        assert!(usage_limited(f.path()));
        let events = activity(f.path(), 10);
        assert_eq!(events.last().map(|e| e.kind), Some(ActivityKind::Error));
    }

    /// Shapes captured from `codex exec --json` 0.162.1 on 2026-10-14: a
    /// command is reported when it starts and again when it completes.
    #[test]
    fn codex_command_shows_once_per_run() {
        let started = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"npm test","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
        let completed = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"npm test","aggregated_output":"ok","exit_code":0,"status":"completed"}}"#;
        let f = log_with(&[
            r#"{"type":"thread.started","thread_id":"a"}"#,
            started,
            completed,
            // A continuation appends a new run whose ids start over.
            r#"{"type":"thread.started","thread_id":"b"}"#,
            started,
            completed,
        ]);
        let tools: Vec<String> = activity(f.path(), 20)
            .into_iter()
            .filter(|e| e.kind == ActivityKind::Tool)
            .map(|e| e.text)
            .collect();
        assert_eq!(tools, ["run npm test", "run npm test"]);
    }

    #[test]
    fn truncates_long_text() {
        let long = "a".repeat(1000);
        let f = log_with(&[&format!(r#"{{"type":"result","result":"{long}"}}"#)]);
        assert_eq!(
            last_activity(f.path()).unwrap().chars().count(),
            MAX_ACTIVITY_CHARS
        );
    }

    #[test]
    fn skips_partial_first_line_of_a_tail_read() {
        let mut lines = vec![format!(
            r#"{{"type":"result","result":"{}"}}"#,
            "x".repeat(70_000)
        )];
        lines.push(r#"{"type":"system"}"#.to_string());
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let f = log_with(&refs);
        assert_eq!(last_activity(f.path()), None);
    }

    #[test]
    fn codex_reasoning_is_never_shown() {
        let f = log_with(&[
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Patched parser"}}"#,
            r#"{"type":"item.completed","item":{"type":"reasoning","text":"secret thoughts"}}"#,
        ]);
        assert_eq!(last_activity(f.path()).as_deref(), Some("Patched parser"));
    }

    #[test]
    fn claude_command_ignores_repository_settings() {
        let cmd = command(
            &AgentPrograms::default(),
            Launch {
                agent: TaskAgent::ClaudeCode,
                permission: TaskPermission::Edits,
                worktree: Path::new("/tmp"),
                prompt: "hi",
                resume_session: None,
                model: None,
                effort: None,
                provider: None,
            },
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let at = args.iter().position(|a| a == "--setting-sources").unwrap();
        assert_eq!(args[at + 1], "user,local");
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
    }

    #[test]
    fn claude_command_never_bypasses_permissions() {
        let cmd = command(
            &AgentPrograms::default(),
            Launch {
                agent: TaskAgent::ClaudeCode,
                permission: TaskPermission::Edits,
                worktree: Path::new("/tmp"),
                prompt: "hi",
                resume_session: None,
                model: None,
                effort: None,
                provider: None,
            },
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(!args
            .iter()
            .any(|a| a.contains("dangerously") || a == "bypassPermissions"));
        assert_eq!(args.last().map(String::as_str), Some("hi"));
    }

    #[test]
    fn model_and_effort_flags_are_passed_before_the_prompt() {
        let claude = args(Launch {
            agent: TaskAgent::ClaudeCode,
            permission: TaskPermission::Edits,
            worktree: Path::new("/w"),
            prompt: "do it",
            resume_session: None,
            model: Some("opus"),
            effort: Some(TaskEffort::High),
            provider: None,
        });
        let model = claude.iter().position(|x| x == "--model").unwrap();
        assert_eq!(claude[model + 1], "opus");
        let effort = claude.iter().position(|x| x == "--effort").unwrap();
        assert_eq!(claude[effort + 1], "high");
        assert_eq!(&claude[claude.len() - 2..], ["--", "do it"]);

        let codex = args(Launch {
            agent: TaskAgent::Codex,
            permission: TaskPermission::Edits,
            worktree: Path::new("/w"),
            prompt: "do it",
            resume_session: None,
            model: Some("gpt-6-astra"),
            effort: Some(TaskEffort::Max),
            provider: None,
        });
        let model = codex.iter().position(|x| x == "--model").unwrap();
        assert_eq!(codex[model + 1], "gpt-6-astra");
        let cfg = codex.iter().position(|x| x == "-c").unwrap();
        assert_eq!(codex[cfg + 1], "model_reasoning_effort=\"high\"");
        assert_eq!(&codex[codex.len() - 2..], ["--", "do it"]);
    }

    #[test]
    fn parse_model_rejects_flag_like_values() {
        assert_eq!(parse_model(None).unwrap(), None);
        assert_eq!(parse_model(Some("  ")).unwrap(), None);
        assert_eq!(parse_model(Some("opus")).unwrap().as_deref(), Some("opus"));
        assert_eq!(
            parse_model(Some("claude-fable-5")).unwrap().as_deref(),
            Some("claude-fable-5")
        );
        assert!(parse_model(Some("--dangerously-skip-permissions")).is_err());
        assert!(parse_model(Some("opus 5")).is_err());
        assert!(parse_model(Some("../evil")).is_err());
    }

    fn provider(kind: ProviderKind, api_key: Option<&str>) -> ProviderLaunch {
        ProviderLaunch {
            id: "openrouter".into(),
            name: "Open \"Router\"".into(),
            kind,
            base_url: (kind == ProviderKind::Custom).then(|| "https://openrouter.ai/api/v1".into()),
            api_key: api_key.map(str::to_string),
        }
    }

    fn codex_with(provider: &ProviderLaunch) -> Command {
        command(
            &AgentPrograms::default(),
            Launch {
                agent: TaskAgent::Codex,
                permission: TaskPermission::Edits,
                worktree: Path::new("/w"),
                prompt: "do it",
                resume_session: None,
                model: Some("qwen/qwen3-coder"),
                effort: None,
                provider: Some(provider),
            },
        )
    }

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn local_providers_use_codex_oss() {
        for (kind, name) in [
            (ProviderKind::Ollama, "ollama"),
            (ProviderKind::Lmstudio, "lmstudio"),
        ] {
            let args = argv(&codex_with(&provider(kind, None)));
            assert!(
                args.windows(3)
                    .any(|w| w == ["--oss", "--local-provider", name]),
                "{args:?}"
            );
            assert!(
                args.windows(2)
                    .any(|w| w == ["--model", "qwen/qwen3-coder"]),
                "{args:?}"
            );
            assert_eq!(&args[args.len() - 2..], ["--", "do it"]);
        }
    }

    #[test]
    fn a_custom_provider_is_defined_for_the_run_and_its_key_stays_out_of_argv() {
        let cmd = codex_with(&provider(ProviderKind::Custom, Some("sk-secret")));
        let args = argv(&cmd);
        let configs: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "-c")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(
            configs,
            [
                r#"model_providers.lore-openrouter.name="Open \"Router\"""#,
                r#"model_providers.lore-openrouter.base_url="https://openrouter.ai/api/v1""#,
                r#"model_providers.lore-openrouter.wire_api="responses""#,
                r#"model_providers.lore-openrouter.env_key="LORE_PROVIDER_API_KEY""#,
                r#"model_provider="lore-openrouter""#,
            ]
        );
        assert!(!args.iter().any(|a| a.contains("sk-secret")), "{args:?}");
        let env: Vec<_> = cmd.get_envs().collect();
        assert!(env
            .iter()
            .any(|(k, v)| *k == PROVIDER_KEY_ENV && v.is_some_and(|v| v == "sk-secret")));

        let keyless = argv(&codex_with(&provider(ProviderKind::Custom, None)));
        assert!(
            !keyless.iter().any(|a| a.contains("env_key")),
            "{keyless:?}"
        );
    }

    #[test]
    fn toml_strings_escape_quotes_backslashes_and_controls() {
        assert_eq!(toml_string("a\"b\\c\n"), r#""a\"b\\c\u000A""#);
    }

    #[test]
    fn parse_model_takes_local_and_routed_ids_but_nothing_path_like() {
        for ok in [
            "gpt-oss:20b",
            "qwen/qwen3-coder",
            "openai/gpt-oss-20b",
            "deepseek-v3.1",
        ] {
            assert_eq!(parse_model(Some(ok)).unwrap().as_deref(), Some(ok));
        }
        for bad in ["/etc/passwd", ".hidden", "a/../b", "-m", "a b"] {
            assert!(parse_model(Some(bad)).is_err(), "{bad}");
        }
    }
}
