//! The session prefix a replay resumes from.
//!
//! The prefix is the original session's lines before the prompt, moved into
//! the scratch clone: a fresh session id so Claude Code treats it as its own
//! session, and every mention of the original repository rewritten to the
//! clone. Without the rewrite the agent follows absolute paths from earlier
//! turns back into the real repository, where the fix may already have landed.

use std::path::Path;

use serde_json::Value;

use crate::session;

/// The rewritten prefix.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefix {
    /// One JSON line per kept line, each ending in a newline.
    pub text: String,
    /// True when the prefix holds a conversation turn. Claude Code cannot
    /// resume a session without one, so a replay without it starts fresh.
    pub has_messages: bool,
}

/// How to move a line from the original session into the replay.
#[derive(Debug, Clone)]
pub struct Rewrite<'a> {
    pub session_id: &'a str,
    pub scratch: &'a Path,
    /// The original repository's toplevel, when the replay cloned one.
    pub toplevel: Option<&'a Path>,
    /// The home directory, for the `~/...` spelling of the toplevel.
    pub home: Option<&'a Path>,
}

/// The first `prompt_line` lines of `session_bytes`, rewritten. Lines that are
/// not JSON are dropped, as Claude Code would skip them anyway.
#[must_use]
pub fn build(session_bytes: &[u8], prompt_line: usize, rewrite: &Rewrite<'_>) -> Prefix {
    let scratch = rewrite.scratch.to_string_lossy();
    let mut replacements: Vec<(String, &str)> = Vec::new();
    if let Some(top) = rewrite.toplevel {
        replacements.push((top.to_string_lossy().into_owned(), &scratch));
        if let Some(rest) = rewrite.home.and_then(|home| top.strip_prefix(home).ok()) {
            if !rest.as_os_str().is_empty() {
                replacements.push((format!("~/{}", rest.to_string_lossy()), &scratch));
            }
        }
    }
    let mut out = Prefix {
        text: String::new(),
        has_messages: false,
    };
    for line in session::split_lines(session_bytes).take(prompt_line) {
        let Ok(mut entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let kind = entry.get("type").and_then(Value::as_str);
        out.has_messages |= matches!(kind, Some("user" | "assistant"));
        if let Some(obj) = entry.as_object_mut() {
            if obj.contains_key("sessionId") {
                obj.insert("sessionId".into(), rewrite.session_id.into());
            }
            if obj.contains_key("cwd") {
                obj.insert("cwd".into(), scratch.as_ref().into());
            }
        }
        for (from, to) in &replacements {
            replace_in(&mut entry, from, to);
        }
        out.text.push_str(&entry.to_string());
        out.text.push('\n');
    }
    out
}

/// Replace path `from` with `to` in every string and object key of `value`.
fn replace_in(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(s) => {
            if let Some(new) = replace_path(s, from, to) {
                *s = new;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| replace_in(v, from, to)),
        Value::Object(map) => {
            let old = std::mem::take(map);
            for (key, mut v) in old {
                replace_in(&mut v, from, to);
                map.insert(replace_path(&key, from, to).unwrap_or(key), v);
            }
        }
        _ => {}
    }
}

/// `text` with each occurrence of the path `from` replaced, or `None` when
/// there is none. An occurrence counts only where a path component ends, so
/// `/src/lore` does not rewrite `/src/lore-other`.
#[must_use]
pub fn replace_path(text: &str, from: &str, to: &str) -> Option<String> {
    if from.is_empty() || !text.contains(from) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(at) = rest.find(from) {
        let after = &rest[at + from.len()..];
        let ends_component = after
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.')));
        out.push_str(&rest[..at]);
        out.push_str(if ends_component { to } else { from });
        changed |= ends_component;
        rest = after;
    }
    out.push_str(rest);
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn lines(entries: &[Value]) -> Vec<u8> {
        entries
            .iter()
            .map(|e| format!("{e}\n"))
            .collect::<String>()
            .into_bytes()
    }

    fn parsed(prefix: &Prefix) -> Vec<Value> {
        prefix
            .text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn rewrites_session_id_cwd_and_repo_paths_before_the_prompt() {
        let session = lines(&[
            json!({"type": "user", "sessionId": "old", "cwd": "/Users/me/src/app",
                   "message": {"content": "look at /Users/me/src/app/main.rs and ~/src/app/x"}}),
            json!({"type": "assistant", "sessionId": "old",
                   "message": {"content": [{"type": "tool_use", "input":
                       {"file_path": "/Users/me/src/app-other/a", "/Users/me/src/app/k": 1}}]}}),
            json!({"type": "user", "sessionId": "old", "message": {"content": "the prompt"}}),
        ]);
        let (scratch, top, home) = (
            PathBuf::from("/tmp/run/wd"),
            PathBuf::from("/Users/me/src/app"),
            PathBuf::from("/Users/me"),
        );
        let prefix = build(
            &session,
            2,
            &Rewrite {
                session_id: "new",
                scratch: &scratch,
                toplevel: Some(&top),
                home: Some(&home),
            },
        );
        assert!(prefix.has_messages);
        let got = parsed(&prefix);
        assert_eq!(got.len(), 2, "the prompt line and after are not copied");
        assert_eq!(got[0]["sessionId"], "new");
        assert_eq!(got[0]["cwd"], "/tmp/run/wd");
        assert_eq!(
            got[0]["message"]["content"],
            "look at /tmp/run/wd/main.rs and /tmp/run/wd/x"
        );
        assert_eq!(got[1]["sessionId"], "new");
        assert!(got[1].get("cwd").is_none(), "cwd is only set where present");
        let input = &got[1]["message"]["content"][0]["input"];
        assert_eq!(input["file_path"], "/Users/me/src/app-other/a");
        assert_eq!(input["/tmp/run/wd/k"], 1);
    }

    #[test]
    fn a_prefix_without_turns_starts_fresh() {
        let session = lines(&[
            json!({"type": "summary", "summary": "x"}),
            json!({"type": "user", "message": {"content": "the prompt"}}),
        ]);
        let scratch = PathBuf::from("/tmp/wd");
        let rewrite = Rewrite {
            session_id: "new",
            scratch: &scratch,
            toplevel: None,
            home: None,
        };
        let prefix = build(&session, 1, &rewrite);
        assert!(!prefix.has_messages);
        assert_eq!(parsed(&prefix).len(), 1);
        assert!(!build(&session, 0, &rewrite).has_messages);
        assert!(build(b"not json\n", 1, &rewrite).text.is_empty());
    }

    #[test]
    fn replace_path_respects_component_ends() {
        assert_eq!(replace_path("/a/b", "/a/b", "/x").as_deref(), Some("/x"));
        assert_eq!(replace_path("/a/b/c /a/b", "/a/b", "/x").as_deref(), Some("/x/c /x"));
        assert_eq!(replace_path("/a/bc", "/a/b", "/x"), None);
        assert_eq!(replace_path("'/a/b'", "/a/b", "/x").as_deref(), Some("'/x'"));
    }
}
