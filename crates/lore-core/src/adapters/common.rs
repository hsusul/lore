//! Small parsing helpers shared by adapters.

use crate::model::{ParsedMessage, PartKind, Role};

const TITLE_MAX_CHARS: usize = 80;

/// Derive a compact title from the first meaningful user request when the
/// native log does not provide one. Agent/runtime bootstrap messages are
/// deliberately skipped so permissions and injected repository instructions
/// do not become session titles.
pub(crate) fn fallback_title(messages: &[ParsedMessage]) -> Option<String> {
    messages
        .iter()
        .filter(|message| message.role == Role::User && !message.is_sidechain)
        .flat_map(|message| &message.parts)
        .filter(|part| part.kind == PartKind::Text)
        .filter_map(|part| part.text.as_deref())
        .find_map(title_from_text)
}

fn title_from_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    // App-provided rich requests place the actual prompt after this marker.
    let candidate = text
        .split_once("## My request:")
        .map_or(text, |(_, request)| request.trim());

    let bootstrap_prefixes = [
        "<permissions instructions>",
        "# AGENTS.md instructions",
        "<environment_context>",
        "<skill>",
        "<app-context>",
        "<apps_instructions>",
        "<plugins_instructions>",
        "<recommended_plugins>",
    ];
    if bootstrap_prefixes
        .iter()
        .any(|prefix| candidate.starts_with(prefix))
    {
        return None;
    }

    let line = candidate
        .lines()
        .map(str::trim)
        .map(|l| {
            l.trim_start_matches('#')
                .trim_start_matches(['-', '*'])
                .trim()
        })
        .find(|line| !line.is_empty() && !(line.starts_with('<') && line.ends_with('>')))?;

    let mut title = String::with_capacity(TITLE_MAX_CHARS + 4);
    let mut count = 0;
    let mut truncated = false;
    for word in line.split_whitespace() {
        if count > 0 {
            if count >= TITLE_MAX_CHARS {
                truncated = true;
                break;
            }
            title.push(' ');
            count += 1;
        }
        for c in word.chars() {
            if count >= TITLE_MAX_CHARS {
                truncated = true;
                break;
            }
            title.push(c);
            count += 1;
        }
        if truncated {
            break;
        }
    }
    if truncated {
        Some(format!("{}…", title.trim_end()))
    } else {
        Some(title)
    }
}

/// Parse an RFC3339 timestamp to epoch milliseconds.
pub(crate) fn epoch_ms(s: &str) -> Option<i64> {
    let dt = time::OffsetDateTime::parse(s.trim(), &time::format_description::well_known::Rfc3339)
        .ok()?;
    i64::try_from(dt.unix_timestamp_nanos() / 1_000_000).ok()
}

/// Bound a schema token used in a diagnostic (never user content).
pub(crate) fn bounded(s: &str) -> String {
    s.chars().take(40).collect()
}

/// Extract an optional string field from a JSON object.
pub(crate) fn str_field(obj: &serde_json::Value, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// Extract an optional serialized JSON field from a JSON object.
pub(crate) fn json_field(obj: &serde_json::Value, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| serde_json::to_string(v).ok())
}

/// Extract an optional non-negative integer from a JSON object.
pub(crate) fn non_negative_int_field(obj: &serde_json::Value, key: &str) -> Option<i64> {
    obj.get(key)
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_u64().and_then(|u| i64::try_from(u).ok()))
        })
        .filter(|&v| v >= 0)
}

/// Neutralize path traversal, drive prefixes, control characters, and zero-width
/// bytes so a recorded `FileEvent.path` can never represent an escape (`../` or
/// `C:\`) or terminal/filesystem poison. Produces a clean relative path.
pub(crate) fn sanitize_path(raw: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for seg in raw.split(['/', '\\']) {
        let clean_seg = if seg.len() == 2
            && seg.as_bytes()[1] == b':'
            && seg.as_bytes()[0].is_ascii_alphabetic()
        {
            ""
        } else {
            seg
        };
        let filtered: String = clean_seg
            .chars()
            .filter(|c| !c.is_control() && !crate::is_zero_width(*c))
            .collect();
        match filtered.as_str() {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(filtered),
        }
    }
    parts.join("/")
}

/// Count added/removed lines in a unified diff (single-`+`/`-` lines, excluding
/// the `+++`/`---` file headers). Returns `None` for empty input so callers can
/// distinguish "no diff" from "zero changes".
pub(crate) fn unified_diff_line_counts(diff: &str) -> Option<(i64, i64)> {
    if diff.is_empty() {
        return None;
    }
    let mut added = 0_i64;
    let mut removed = 0_i64;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => added = added.saturating_add(1),
            Some(b'-') => removed = removed.saturating_add(1),
            _ => {}
        }
    }
    Some((added, removed))
}

/// After segments exist, assign each derived file event the segment of the
/// message its tool call was invoked in (matched by native call id).
pub(crate) fn resolve_file_event_segments(session: &mut crate::model::ParsedSession) {
    use std::collections::HashMap;
    let call_seq: HashMap<String, i64> = session
        .tool_calls
        .iter()
        .map(|t| (t.native_call_id.clone(), t.call_ref.0))
        .collect();
    let seq_seg: HashMap<i64, usize> = session
        .messages
        .iter()
        .map(|m| (m.seq, m.segment_ix))
        .collect();
    for fe in &mut session.file_events {
        if let Some(id) = &fe.tool_native_call_id {
            if let Some(s) = call_seq.get(id) {
                fe.segment_ix = seq_seg.get(s).copied().unwrap_or(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_traversal() {
        assert_eq!(sanitize_path("../../a/b"), "a/b");
        assert_eq!(sanitize_path(r"..\..\a\b"), "a/b");
        assert_eq!(sanitize_path(r"src\..\src\app.ts"), "src/app.ts");
        assert_eq!(sanitize_path(r"foo/bar\baz"), "foo/bar/baz");
        assert_eq!(sanitize_path("foo///bar\\\\baz"), "foo/bar/baz");
        assert_eq!(sanitize_path("./a/./b/./c.ts"), "a/b/c.ts");
        assert_eq!(sanitize_path("a/b/c/../../d.ts"), "a/d.ts");
        assert_eq!(
            sanitize_path(r"C:\Users\dev\project\file.ts"),
            "Users/dev/project/file.ts"
        );
        assert_eq!(
            sanitize_path(r"d:/project/src/index.js"),
            "project/src/index.js"
        );
        assert_eq!(
            sanitize_path("src/\0poison\r/app\u{200b}.ts"),
            "src/poison/app.ts"
        );
        assert_eq!(
            sanitize_path("\u{feff}src/\x1b[31mred\x1b[0m/\u{2060}main\u{200d}.rs"),
            "src/[31mred[0m/main.rs"
        );
    }

    #[test]
    fn diff_counts_ignore_headers() {
        let diff = "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n ctx";
        assert_eq!(unified_diff_line_counts(diff), Some((1, 1)));
        assert_eq!(unified_diff_line_counts(""), None);

        // Multiple hunks, CRLF line endings, and "\\ No newline at end of file"
        let complex_diff = "--- a/file.rs\r\n+++ b/file.rs\r\n@@ -1,3 +1,4 @@\r\n-first\r\n-second\r\n+first updated\r\n+second updated\r\n+third added\r\n\\ No newline at end of file\r\n";
        assert_eq!(unified_diff_line_counts(complex_diff), Some((3, 2)));

        // Pure addition diff with git headers
        let add_only = "diff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0000000..1234567\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+line 1\n+line 2\n";
        assert_eq!(unified_diff_line_counts(add_only), Some((2, 0)));

        // Single character filenames in diff headers
        let single_char_diff = "--- a/x\n+++ b/x\n@@ -1,1 +1,2 @@\n-old\n+new 1\n+new 2\n";
        assert_eq!(unified_diff_line_counts(single_char_diff), Some((2, 1)));

        // Context only diff (0 additions, 0 deletions)
        let context_only = "--- a/f.rs\n+++ b/f.rs\n@@ -1,3 +1,3 @@\n ctx1\n ctx2\n ctx3\n";
        assert_eq!(unified_diff_line_counts(context_only), Some((0, 0)));

        // Git headers only diff (0 additions, 0 deletions)
        let headers_only = "diff --git a/f.rs b/f.rs\nindex 0000000..1234567\n--- a/f.rs\n+++ b/f.rs\n";
        assert_eq!(unified_diff_line_counts(headers_only), Some((0, 0)));
    }

    #[test]
    fn fallback_title_skips_bootstrap_and_uses_the_real_request() {
        assert_eq!(title_from_text("<permissions instructions>\n…"), None);
        assert_eq!(title_from_text("# AGENTS.md instructions\n…"), None);
        assert_eq!(
            title_from_text("<appshot>…</appshot>\n\n## My request:\nFix the missing titles"),
            Some("Fix the missing titles".to_string())
        );
        assert_eq!(
            title_from_text("<USER_REQUEST>\nFix repository discovery\n</USER_REQUEST>"),
            Some("Fix repository discovery".to_string())
        );
        assert_eq!(title_from_text("<empty_tag>\n</empty_tag>"), None);
        assert_eq!(title_from_text("<environment_context>\nOS: Mac\nCwd: /repo\n"), None);
        assert_eq!(title_from_text("<skill>\nName: rust-dev\n"), None);
        assert_eq!(
            title_from_text("<context>repository info</context>\nRefactor SQLite queries"),
            Some("Refactor SQLite queries".to_string())
        );
    }

    #[test]
    fn fallback_title_is_single_line_and_bounded() {
        assert_eq!(
            title_from_text("  #   Improve   session   titles\nMore detail"),
            Some("Improve session titles".to_string())
        );
        assert_eq!(
            title_from_text("- Add database migrations\nAdditional text"),
            Some("Add database migrations".to_string())
        );
        assert_eq!(
            title_from_text("### Fix authentication token refresh\nBody"),
            Some("Fix authentication token refresh".to_string())
        );
        assert_eq!(
            title_from_text("  * 修复数据库迁移中的并发写入冲突\n详细信息"),
            Some("修复数据库迁移中的并发写入冲突".to_string())
        );
        let title = title_from_text(&"x".repeat(100)).unwrap();
        assert_eq!(title.chars().count(), TITLE_MAX_CHARS + 1);
        assert!(title.ends_with('…'));

        let cjk_title = title_from_text(&"测".repeat(100)).unwrap();
        assert_eq!(cjk_title.chars().count(), TITLE_MAX_CHARS + 1);
        assert!(cjk_title.ends_with('…'));

        // Tab and whitespace collapsing
        assert_eq!(
            title_from_text("\t\t\tRefactor\t\tnetwork module\twith tabs\n"),
            Some("Refactor network module with tabs".to_string())
        );
        assert_eq!(
            title_from_text("\n\n   Update README documentation   \n\n"),
            Some("Update README documentation".to_string())
        );
        assert_eq!(title_from_text("   \t  \n  \r\n  \t  "), None);
        assert_eq!(title_from_text(""), None);

        // Markdown inline backticks and bullet asterisks
        assert_eq!(
            title_from_text("`cargo test` failure in `lore-core`\nstack trace"),
            Some("`cargo test` failure in `lore-core`".to_string())
        );
        assert_eq!(
            title_from_text("* Implement background indexing worker\nDetails"),
            Some("Implement background indexing worker".to_string())
        );

        // CRLF line endings
        assert_eq!(
            title_from_text("\r\n\r\nFix windows CRLF line endings\r\nSecond line"),
            Some("Fix windows CRLF line endings".to_string())
        );
    }

    #[test]
    fn non_negative_int_field_validates_bounds() {
        let json: serde_json::Value = serde_json::json!({
            "zero": 0,
            "neg_zero": -0,
            "positive": 42,
            "negative": -5,
            "string": "123",
            "null_val": null,
            "i64_max": i64::MAX,
            "u64_overflow": u64::MAX,
            "float": 12.34,
            "bool": true
        });
        assert_eq!(non_negative_int_field(&json, "zero"), Some(0));
        assert_eq!(non_negative_int_field(&json, "neg_zero"), Some(0));
        assert_eq!(non_negative_int_field(&json, "positive"), Some(42));
        assert_eq!(non_negative_int_field(&json, "negative"), None);
        assert_eq!(non_negative_int_field(&json, "string"), None);
        assert_eq!(non_negative_int_field(&json, "null_val"), None);
        assert_eq!(non_negative_int_field(&json, "missing"), None);
        assert_eq!(non_negative_int_field(&json, "i64_max"), Some(i64::MAX));
        assert_eq!(non_negative_int_field(&json, "u64_overflow"), None);
        assert_eq!(non_negative_int_field(&json, "float"), None);
        assert_eq!(non_negative_int_field(&json, "bool"), None);
    }

    #[test]
    fn json_and_str_field_extract_values() {
        let json: serde_json::Value = serde_json::json!({
            "name": "test-adapter",
            "nested": { "key": "value" },
            "empty_str": "",
            "boolean": true,
            "arr": [1, 2, 3],
            "null_field": null
        });
        assert_eq!(str_field(&json, "name"), Some("test-adapter".to_string()));
        assert_eq!(str_field(&json, "empty_str"), Some("".to_string()));
        assert_eq!(str_field(&json, "nested"), None);
        assert_eq!(str_field(&json, "boolean"), None);
        assert_eq!(str_field(&json, "arr"), None);
        assert_eq!(str_field(&json, "null_field"), None);
        assert_eq!(str_field(&json, "missing"), None);

        assert_eq!(
            json_field(&json, "nested"),
            Some("{\"key\":\"value\"}".to_string())
        );
        assert_eq!(
            json_field(&json, "name"),
            Some("\"test-adapter\"".to_string())
        );
        assert_eq!(json_field(&json, "boolean"), Some("true".to_string()));
        assert_eq!(json_field(&json, "arr"), Some("[1,2,3]".to_string()));
        assert_eq!(json_field(&json, "null_field"), Some("null".to_string()));
        assert_eq!(json_field(&json, "missing"), None);
    }

    #[test]
    fn epoch_ms_parses_rfc3339_with_optional_whitespace() {
        let ts = "2026-08-11T10:00:00.000Z";
        let expected = epoch_ms(ts);
        assert!(expected.is_some());
        assert_eq!(epoch_ms(&format!("  {ts}  ")), expected);
        assert_eq!(epoch_ms("not-a-date"), None);
        assert_eq!(epoch_ms(""), None);

        // Explicit positive and negative timezone offsets
        let offset_ts = "2026-08-11T12:00:00+02:00";
        assert_eq!(epoch_ms(offset_ts), expected);
        let negative_offset_ts = "2026-08-11T05:00:00-05:00";
        assert_eq!(epoch_ms(negative_offset_ts), expected);

        // Subsecond millisecond precision
        let base = expected.unwrap();
        let ms_ts = "2026-08-11T10:00:00.123Z";
        assert_eq!(epoch_ms(ms_ts), Some(base + 123));

        let micros_ts = "2026-08-11T10:00:00.123456Z";
        assert_eq!(epoch_ms(micros_ts), Some(base + 123));

        // Leap year and invalid calendar date handling
        assert!(epoch_ms("2024-02-29T12:00:00Z").is_some());
        assert_eq!(epoch_ms("2025-02-29T12:00:00Z"), None);
        assert_eq!(epoch_ms("2026-13-01T00:00:00Z"), None);
        assert_eq!(epoch_ms("2026-08-11T25:00:00Z"), None);
        assert_eq!(epoch_ms("2026-08-11T10:65:00Z"), None);
        assert_eq!(epoch_ms("2026-08-11T10:00:65Z"), None);
        assert_eq!(epoch_ms("2026-08-11T10:00:00+25:00"), None);

        // UTC offset zero representations and tab/newline trimming
        let plus_zero = "2026-08-11T10:00:00+00:00";
        assert_eq!(epoch_ms(plus_zero), expected);
        let minus_zero = "2026-08-11T10:00:00-00:00";
        assert_eq!(epoch_ms(minus_zero), expected);
        let tabs_ts = "\t\n2026-08-11T10:00:00.000Z\n\t";
        assert_eq!(epoch_ms(tabs_ts), expected);
    }

    #[test]
    fn sanitize_path_neutralizes_traversal_and_drive_letters() {
        assert_eq!(sanitize_path("../../foo/bar.rs"), "foo/bar.rs");
        assert_eq!(
            sanitize_path("C:\\Users\\test\\file.txt"),
            "Users/test/file.txt"
        );
        assert_eq!(sanitize_path("./src/./main.rs"), "src/main.rs");
        assert_eq!(sanitize_path("/absolute/path/file.rs"), "absolute/path/file.rs");
        assert_eq!(sanitize_path("a/b/c/../../d.rs"), "a/d.rs");
        assert_eq!(sanitize_path("src/app/"), "src/app");
        assert_eq!(sanitize_path(r"src\app\"), "src/app");
        assert_eq!(sanitize_path(r"\\server\share\file.rs"), "server/share/file.rs");
        assert_eq!(sanitize_path(".../src/lib.rs"), ".../src/lib.rs");
    }

    #[test]
    fn bounded_safely_truncates_multibyte_characters() {
        let ascii = "a".repeat(100);
        assert_eq!(bounded(&ascii), "a".repeat(40));
        let cjk = "测".repeat(100);
        assert_eq!(bounded(&cjk), "测".repeat(40));
        let short = "custom_event";
        assert_eq!(bounded(short), "custom_event");
        assert_eq!(bounded(""), "");
    }
}
