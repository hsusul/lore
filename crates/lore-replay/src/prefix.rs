//! The session prefix a replay resumes from.
//!
//! The prefix is the original session's lines before the prompt, moved into
//! the scratch clone: a fresh session id so Claude Code treats it as its own
//! session, and every mention of the original repository rewritten to the
//! clone. Without the rewrite the agent follows absolute paths from earlier
//! turns back into the real repository, where the fix may already have landed.
//!
//! With [`Rewrite::trim`] set, old tool output and images are replaced with
//! short placeholders (see [`trim_line`]): most of a long session's prefix is
//! tool output the agent read long ago, and every arm's first run writes the
//! whole prefix into the prompt cache. Trimming is a pure function of the
//! session, so every arm of a case still shares one cache prefix.

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
    /// Keep the last `trim` user and assistant lines whole and trim the tool
    /// output and images of every earlier one. 0 trims nothing.
    pub trim: usize,
}

/// The text an omitted image becomes.
pub const IMAGE_OMITTED: &str = "[image omitted in replay]";

/// The text omitted tool output becomes.
#[must_use]
pub fn tool_output_omitted(chars: usize) -> String {
    format!("[tool output omitted in replay: {chars} characters]")
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
    let entries: Vec<Value> = session::split_lines(session_bytes)
        .take(prompt_line)
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .collect();
    let turns = entries.iter().filter(|e| is_turn(e)).count();
    // Turn lines before this many are trimmed; the rest are kept whole.
    let trim_before = if rewrite.trim == 0 {
        0
    } else {
        turns.saturating_sub(rewrite.trim)
    };
    let mut turn = 0;
    for mut entry in entries {
        if is_turn(&entry) {
            out.has_messages = true;
            if turn < trim_before {
                trim_line(&mut entry);
            }
            turn += 1;
        }
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

/// A user or assistant line: a conversation turn.
fn is_turn(entry: &Value) -> bool {
    matches!(
        entry.get("type").and_then(Value::as_str),
        Some("user" | "assistant")
    )
}

/// Trim one user or assistant line: each `tool_result` block's content
/// becomes [`tool_output_omitted`] with the original length in characters
/// (its `tool_use_id` and `is_error` stay, so the result still pairs with its
/// `tool_use`), and each image block, top level or inside a tool result,
/// becomes a text block saying [`IMAGE_OMITTED`]. Text, `tool_use`, and every
/// other field are left alone. A tool result with nothing in it is kept.
pub fn trim_line(entry: &mut Value) {
    let Some(blocks) = entry
        .pointer_mut("/message/content")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("image") => *block = image_placeholder(),
            Some("tool_result") => {
                if let Some(content) = block.get_mut("content") {
                    trim_tool_content(content);
                }
            }
            _ => {}
        }
    }
}

fn image_placeholder() -> Value {
    serde_json::json!({"type": "text", "text": IMAGE_OMITTED})
}

/// A tool result's content, trimmed: a string becomes the placeholder; an
/// array becomes one placeholder for its non-image blocks, then one image
/// placeholder per image it held.
fn trim_tool_content(content: &mut Value) {
    match content {
        Value::String(s) if !s.is_empty() => *s = tool_output_omitted(s.chars().count()),
        Value::Array(items) if !items.is_empty() => {
            let mut chars = 0;
            let mut others = 0;
            let mut images = 0;
            for item in items.iter() {
                match item.get("type").and_then(Value::as_str) {
                    Some("image") => images += 1,
                    Some("text") => {
                        others += 1;
                        chars += item
                            .get("text")
                            .and_then(Value::as_str)
                            .map_or(0, |t| t.chars().count());
                    }
                    _ => {
                        others += 1;
                        chars += item.to_string().chars().count();
                    }
                }
            }
            let mut trimmed = Vec::with_capacity(1 + images);
            if others > 0 {
                trimmed
                    .push(serde_json::json!({"type": "text", "text": tool_output_omitted(chars)}));
            }
            trimmed.extend((0..images).map(|_| image_placeholder()));
            *items = trimmed;
        }
        _ => {}
    }
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
                trim: 0,
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
            trim: 0,
        };
        let prefix = build(&session, 1, &rewrite);
        assert!(!prefix.has_messages);
        assert_eq!(parsed(&prefix).len(), 1);
        assert!(!build(&session, 0, &rewrite).has_messages);
        assert!(build(b"not json\n", 1, &rewrite).text.is_empty());
    }

    /// A session with tool output and images in old and recent turns, then
    /// the prompt.
    fn tool_session() -> Vec<Value> {
        let png = json!({"type": "image", "source": {"type": "base64", "data": "AAAA"}});
        vec![
            json!({"type": "summary", "summary": "s"}),
            json!({"type": "user", "sessionId": "old", "cwd": "/r",
                   "message": {"content": [{"type": "text", "text": "look"}, png]}}),
            json!({"type": "assistant", "sessionId": "old", "message": {"content": [
                {"type": "text", "text": "reading"},
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/r/a"}}]}}),
            json!({"type": "user", "sessionId": "old", "toolUseResult": {"stdout": "héllo"},
                   "message": {"content": [{"type": "tool_result", "tool_use_id": "t1",
                                              "is_error": true, "content": "héllo"}]}}),
            json!({"type": "system", "content": "not a turn"}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "t2", "name": "Shot", "input": {}}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result",
                "tool_use_id": "t2", "content": [{"type": "text", "text": "abc"}, png]}]}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "t3", "name": "Bash", "input": {}}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result",
                "tool_use_id": "t3", "content": [{"type": "text", "text": "recent"}, png]}]}}),
            json!({"type": "user", "message": {"content": "the prompt"}}),
        ]
    }

    fn trimmed(session: &[u8], trim: usize) -> Prefix {
        let scratch = PathBuf::from("/tmp/wd");
        build(
            session,
            9,
            &Rewrite {
                session_id: "new",
                scratch: &scratch,
                toplevel: Some(Path::new("/r")),
                home: None,
                trim,
            },
        )
    }

    #[test]
    fn trim_replaces_only_old_tool_output_and_images() {
        let session = lines(&tool_session());
        let whole = trimmed(&session, 0);
        let prefix = trimmed(&session, 2);
        assert_eq!(prefix, trimmed(&session, 2), "trimming is deterministic");
        let (got, before) = (parsed(&prefix), parsed(&whole));
        assert_eq!(got.len(), 9, "no line is added or dropped");
        assert_eq!(got.len(), before.len());
        for (g, b) in got.iter().zip(&before) {
            assert_eq!(g["type"], b["type"], "lines keep their order");
        }

        // The image in the first user line becomes text; its text stays.
        let first = &got[1]["message"]["content"];
        assert_eq!(first[0], json!({"type": "text", "text": "look"}));
        assert_eq!(first[1], json!({"type": "text", "text": IMAGE_OMITTED}));
        assert_eq!(got[1]["sessionId"], "new");
        assert_eq!(got[1]["cwd"], "/tmp/wd");
        // Assistant text and tool_use are untouched, paths still rewritten.
        assert_eq!(got[2], before[2]);
        assert_eq!(
            got[2]["message"]["content"][1]["input"]["file_path"],
            "/tmp/wd/a"
        );
        // A string tool result: length in characters, ids and is_error kept.
        assert_eq!(
            got[3]["message"]["content"][0],
            json!({"type": "tool_result", "tool_use_id": "t1", "is_error": true,
                   "content": "[tool output omitted in replay: 5 characters]"})
        );
        assert_eq!(got[3]["toolUseResult"], before[3]["toolUseResult"]);
        assert_eq!(got[4], before[4], "a non-turn line is left alone");
        assert_eq!(got[5], before[5]);
        // An array tool result: one placeholder for the text, one per image.
        assert_eq!(
            got[6]["message"]["content"][0],
            json!({"type": "tool_result", "tool_use_id": "t2", "content": [
                {"type": "text", "text": "[tool output omitted in replay: 3 characters]"},
                {"type": "text", "text": IMAGE_OMITTED}]})
        );
        // The last two turn lines are byte-identical to the untrimmed prefix.
        let (tail, whole_tail): (Vec<&str>, Vec<&str>) = (
            prefix.text.lines().skip(7).collect(),
            whole.text.lines().skip(7).collect(),
        );
        assert_eq!(tail, whole_tail);
        assert_eq!(tail.len(), 2);
    }

    #[test]
    fn trim_zero_or_above_the_turn_count_changes_nothing() {
        let session = lines(&tool_session());
        let whole = trimmed(&session, 0);
        assert_eq!(trimmed(&session, 7), whole);
        assert_eq!(trimmed(&session, 100), whole);
        assert_ne!(trimmed(&session, 6), whole);
    }

    #[test]
    fn replace_path_respects_component_ends() {
        assert_eq!(replace_path("/a/b", "/a/b", "/x").as_deref(), Some("/x"));
        assert_eq!(
            replace_path("/a/b/c /a/b", "/a/b", "/x").as_deref(),
            Some("/x/c /x")
        );
        assert_eq!(replace_path("/a/bc", "/a/b", "/x"), None);
        assert_eq!(
            replace_path("'/a/b'", "/a/b", "/x").as_deref(),
            Some("'/x'")
        );
    }
}
