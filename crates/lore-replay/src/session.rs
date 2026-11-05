//! Reading a Claude Code session file line by line.
//!
//! Line numbers are the replay's coordinates: a case names the prompt and
//! correction by line, and the prefix is every line before the prompt. So an
//! unparseable line still occupies its index, as [`Value::Null`].

use std::path::Path;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::types::Action;

/// Every line of `path`, parsed. Invalid UTF-8 is replaced rather than
/// rejected, and a line that is not JSON reads as `Null`.
pub fn read_lines(path: &Path) -> Result<Vec<Value>> {
    let bytes = std::fs::read(path).map_err(|e| Error::io("read session file", path, e))?;
    Ok(split_lines(&bytes)
        .map(|line| serde_json::from_slice(line).unwrap_or(Value::Null))
        .collect())
}

/// The file's lines without their terminators. A final newline does not start
/// another line.
pub fn split_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let empty = body.is_empty() && bytes.is_empty();
    body.split(|b| *b == b'\n').filter(move |_| !empty)
}

fn str_field<'a>(entry: &'a Value, key: &str) -> Option<&'a str> {
    entry.get(key).and_then(Value::as_str)
}

fn flag(entry: &Value, key: &str) -> bool {
    entry.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn content(entry: &Value) -> Option<&Value> {
    entry.get("message").and_then(|m| m.get("content"))
}

fn parts(entry: &Value) -> impl Iterator<Item = &serde_json::Map<String, Value>> {
    content(entry)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
}

fn part_is(part: &serde_json::Map<String, Value>, kind: &str) -> bool {
    part.get("type").and_then(Value::as_str) == Some(kind)
}

/// The text a message carries: the whole string, or its text parts joined by
/// newlines.
#[must_use]
pub fn content_text(entry: &Value) -> String {
    match content(entry) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(_)) => parts(entry)
            .filter(|p| part_is(p, "text"))
            .map(|p| p.get("text").and_then(Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// True for a turn a person typed: not a tool result, not a sidechain or meta
/// line, not a compaction summary or a command wrapper.
#[must_use]
pub fn is_human(entry: &Value) -> bool {
    if str_field(entry, "type") != Some("user")
        || flag(entry, "isSidechain")
        || flag(entry, "isMeta")
    {
        return false;
    }
    match content(entry) {
        Some(Value::String(s)) => {
            !s.starts_with("This session is being continued") && !s.starts_with('<')
        }
        Some(Value::Array(_)) => {
            parts(entry).any(|p| part_is(p, "text"))
                && !parts(entry).any(|p| part_is(p, "tool_result"))
        }
        _ => false,
    }
}

/// What the main-thread assistant did across `entries`: non-blank prose and
/// tool calls, in order.
#[must_use]
pub fn assistant_actions(entries: &[Value]) -> Vec<Action> {
    let mut out = Vec::new();
    for entry in entries {
        if str_field(entry, "type") != Some("assistant") || flag(entry, "isSidechain") {
            continue;
        }
        for part in parts(entry) {
            if part_is(part, "text") {
                let text = part.get("text").and_then(Value::as_str).unwrap_or("");
                if !text.trim().is_empty() {
                    out.push(Action::Text {
                        text: text.to_owned(),
                    });
                }
            } else if part_is(part, "tool_use") {
                if let Some(name) = part.get("name").and_then(Value::as_str) {
                    out.push(Action::Tool {
                        name: name.to_owned(),
                        input: part.get("input").cloned().unwrap_or(Value::Null),
                    });
                }
            }
        }
    }
    out
}

/// The line's `timestamp` as epoch milliseconds.
#[must_use]
pub fn timestamp_ms(entry: &Value) -> Option<i64> {
    let ts = str_field(entry, "timestamp")?;
    let at =
        time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339).ok()?;
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()
}

#[must_use]
pub fn cwd(entry: &Value) -> Option<&str> {
    str_field(entry, "cwd").filter(|s| !s.is_empty())
}

#[must_use]
pub fn branch(entry: &Value) -> Option<&str> {
    str_field(entry, "gitBranch").filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lines_keep_their_index_and_a_final_newline_adds_none() {
        assert_eq!(split_lines(b"a\nb\n").count(), 2);
        assert_eq!(split_lines(b"a\nb").count(), 2);
        assert_eq!(split_lines(b"").count(), 0);
        assert_eq!(split_lines(b"\n").count(), 1);
    }

    #[test]
    fn tool_results_and_wrappers_are_not_human_turns() {
        let typed = json!({"type": "user", "message": {"content": "fix it"}});
        let wrapper = json!({"type": "user", "message": {"content": "<command-name>"}});
        let result = json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "content": "ok"}, {"type": "text", "text": "x"}]}});
        let meta = json!({"type": "user", "isMeta": true, "message": {"content": "x"}});
        assert!(is_human(&typed));
        assert!(!is_human(&wrapper));
        assert!(!is_human(&result));
        assert!(!is_human(&meta));
    }

    #[test]
    fn actions_skip_blank_text_and_sidechains() {
        let entries = [
            json!({"type": "assistant", "message": {"content": [
                {"type": "text", "text": "  "},
                {"type": "text", "text": "doing it"},
                {"type": "tool_use", "name": "Bash", "input": {"command": "ls"}}]}}),
            json!({"type": "assistant", "isSidechain": true, "message": {"content": [
                {"type": "text", "text": "hidden"}]}}),
        ];
        assert_eq!(
            assistant_actions(&entries),
            vec![
                Action::Text {
                    text: "doing it".into()
                },
                Action::Tool {
                    name: "Bash".into(),
                    input: json!({"command": "ls"})
                },
            ]
        );
    }

    #[test]
    fn timestamps_read_as_epoch_ms() {
        let e = json!({"timestamp": "2026-01-02T03:04:05.678Z"});
        assert_eq!(timestamp_ms(&e), Some(1_767_323_045_678));
    }
}
