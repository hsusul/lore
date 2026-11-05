//! One-shot questions to a model through `claude -p`, for labeling cases and
//! judging runs. **This talks to a model provider**, under the person's login.
//!
//! The judge gets the same no-memory base as a replay: no CLAUDE.md, no auto
//! memory, no settings, no MCP, no tools, no saved session. The prompt goes on
//! stdin: `--tools ""` is variadic, so a prompt passed as an argument after it
//! would be read as a tool name.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::run::CLAUDE_ENV;

/// The `labeled_by` / `graded_by` name of a judge model.
#[must_use]
pub fn judge_name(model: &str) -> String {
    format!("judge: claude {model}")
}

/// Ask `model` one question and return its text reply.
///
/// A failed start, a non-zero exit, an `is_error` result (a rate limit, for
/// one), or output without a `result` is an error, so callers stop and a later
/// run resumes instead of recording a non-answer as an answer.
pub fn ask(model: &str, prompt: &str) -> Result<String> {
    let program = std::env::var_os(CLAUDE_ENV).unwrap_or_else(|| "claude".into());
    let mut child = Command::new(&program)
        .args([
            "-p",
            "--model",
            model,
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--tools",
            "",
            "--output-format",
            "json",
        ])
        // Away from any project, so no project file is in reach.
        .current_dir(std::env::temp_dir())
        .env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1")
        .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Spawn {
            program: program.to_string_lossy().into_owned(),
            source,
        })?;
    let writer = child.stdin.take().map(|mut pipe| {
        let prompt = prompt.to_owned();
        std::thread::spawn(move || {
            // A closed pipe means the judge stopped reading; its exit says why.
            let _ = pipe.write_all(prompt.as_bytes());
        })
    });
    let out = child
        .wait_with_output()
        .map_err(|e| Error::io("wait for", std::path::Path::new(&program), e))?;
    if let Some(handle) = writer {
        let _ = handle.join();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reply: Option<Value> = serde_json::from_str(stdout.trim()).ok();
    let result = reply
        .as_ref()
        .and_then(|r| r.get("result"))
        .and_then(Value::as_str);
    let is_error = reply
        .as_ref()
        .and_then(|r| r.get("is_error"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match result {
        Some(text) if out.status.success() && !is_error => Ok(text.to_owned()),
        _ => Err(Error::Judge(format!(
            "exit {}; stdout: {}; stderr: {}",
            out.status,
            head(&stdout, 300),
            head(&stderr, 300)
        ))),
    }
}

/// The first `chars` characters of `s`.
#[must_use]
pub fn head(s: &str, chars: usize) -> String {
    s.chars().take(chars).collect()
}
