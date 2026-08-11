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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_traversal() {
        assert_eq!(sanitize_path("../../a/b"), "a/b");
    }
}
