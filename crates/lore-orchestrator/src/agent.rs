//! How each agent CLI is launched headless, and how its output is summarized.
//!
//! Launch flags were verified against `claude --help` (2.1.251) and
//! `codex exec --help` (0.144.6) on 2026-09-12. Lore never passes a flag that
//! bypasses the agent's permission prompts or sandbox (ADR-0007).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

use lore_ipc::{ActivityDto, ActivityKind, TaskAgent, TaskPermission};

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
#[derive(Debug, Clone, Copy)]
pub struct Launch<'a> {
    pub agent: TaskAgent,
    pub permission: TaskPermission,
    pub worktree: &'a Path,
    pub prompt: &'a str,
    /// Resume this agent session instead of starting a new one (Claude only).
    pub resume_session: Option<&'a str>,
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
            cmd.arg("--").arg(launch.prompt);
            cmd
        }
        TaskAgent::Codex => {
            let mut cmd = Command::new(&programs.codex);
            // workspace-write confines writes to the worktree via Codex's sandbox.
            // `codex exec resume` (0.144.6) accepts neither --sandbox nor --cd, so
            // Codex continuations start a new session with a brief instead.
            cmd.args(["exec", "--json", "--sandbox", "workspace-write", "--cd"])
                .arg(launch.worktree)
                .arg("--")
                .arg(launch.prompt);
            cmd
        }
    };
    cmd.current_dir(launch.worktree);
    cmd
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
            let lower = e.text.to_lowercase();
            if lower.contains("usage limit")
                || lower.contains("rate limit")
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
                .find(|l| !l.starts_with('{'))
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
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => events.extend(activity_from_event(&v)),
            Err(_) if !line.starts_with('{') => events.push(ActivityDto {
                kind: if line.to_lowercase().contains("error") {
                    ActivityKind::Error
                } else {
                    ActivityKind::Output
                },
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
                    _ => {}
                }
            } else if let Some(s) = v.pointer("/error/message").and_then(|s| s.as_str()) {
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
                    ActivityKind::Error,
                    "hook failed: connect ECONNREFUSED error"
                ),
            ]
        );
        assert_eq!(activity(f.path(), 2).len(), 2);
        assert!(!events
            .iter()
            .any(|e| e.text.contains("private") || e.text.contains("whole file")));
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
        };
        let a = args(base);
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
}
