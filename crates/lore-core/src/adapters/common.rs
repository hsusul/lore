//! Small parsing helpers shared by adapters.

/// Parse an RFC3339 timestamp to epoch milliseconds.
pub(crate) fn epoch_ms(s: &str) -> Option<i64> {
    let dt = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()?;
    i64::try_from(dt.unix_timestamp_nanos() / 1_000_000).ok()
}

/// Bound a schema token used in a diagnostic (never user content).
pub(crate) fn bounded(s: &str) -> String {
    s.chars().take(40).collect()
}

/// Neutralize path traversal so a recorded `FileEvent.path` can never represent
/// an escape (`../`). Produces a clean relative path.
pub(crate) fn sanitize_path(raw: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
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
            Some(b'+') => added += 1,
            Some(b'-') => removed += 1,
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
    }

    #[test]
    fn diff_counts_ignore_headers() {
        let diff = "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n ctx";
        assert_eq!(unified_diff_line_counts(diff), Some((1, 1)));
        assert_eq!(unified_diff_line_counts(""), None);
    }
}
