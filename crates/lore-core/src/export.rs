//! Redaction-aware session export (`SECURITY.md` §4, §6).
//!
//! A default export **masks** every flagged secret span, so exporting cannot
//! amplify a secret out of the archive. Including flagged content requires an
//! explicit, per-call override (`include_secrets`) — there is no sticky setting.
//! Opaque/encrypted regions are never decoded or exported. Rendering reuses the
//! canonical read path (`query::get_session`) and the same scanner as ingest.

use std::fmt::Write;

use rusqlite::Connection;

use crate::query;
use crate::secrets;
use crate::storage::Result;

/// Render a session as Markdown. When `include_secrets` is false (the default
/// posture) every flagged secret span is masked; `true` is an explicit opt-in to
/// full-fidelity content. Returns `None` when the session is unknown.
pub fn export_session_markdown(
    conn: &Connection,
    session_id: &str,
    include_secrets: bool,
) -> Result<Option<String>> {
    let Some(detail) = query::get_session(conn, session_id)? else {
        return Ok(None);
    };
    let s = &detail.summary;
    let estimated_capacity = detail
        .messages
        .len()
        .saturating_mul(256)
        .saturating_add(detail.file_events.len().saturating_mul(64))
        .saturating_add(512);
    let mut out = String::with_capacity(estimated_capacity);
    let render = |text: &str| render_field(text, include_secrets);

    let title = s.title.as_deref().unwrap_or("(untitled session)");
    let clean_title = title.replace(['\r', '\n'], " ");
    let _ = writeln!(out, "# {}", render(&clean_title));
    let _ = writeln!(
        out,
        "\n> {} · {} messages · {} tool calls{}\n",
        s.agent_id,
        s.message_count,
        s.tool_call_count,
        if include_secrets {
            " · ⚠ includes flagged secrets"
        } else {
            ""
        }
    );

    for message in &detail.messages {
        let _ = writeln!(out, "### {}", message.role);
        for part in &message.parts {
            match part.kind.as_str() {
                "opaque" => {
                    let _ = writeln!(out, "_[encrypted content omitted]_\n");
                }
                "thinking" => {
                    if let Some(text) = &part.text {
                        let _ = writeln!(out, "> _(thinking)_ {}\n", render(text));
                    }
                }
                _ => {
                    if let Some(text) = &part.text {
                        let _ = writeln!(out, "{}\n", render(text));
                    } else if let Some(json) = &part.content_json {
                        let rendered_json = render(json);
                        let fence = if rendered_json.contains("```") {
                            "````"
                        } else {
                            "```"
                        };
                        let _ = writeln!(out, "{fence}json\n{rendered_json}\n{fence}\n");
                    }
                }
            }
        }
    }

    if !detail.file_events.is_empty() {
        let _ = writeln!(out, "### Files");
        for file in &detail.file_events {
            let clean_path = file.path.replace('`', "'").replace(['\r', '\n'], " ");
            let _ = writeln!(out, "- `{clean_path}` ({})", file.change_kind);
        }
    }

    Ok(Some(out))
}

/// Mask flagged spans unless the caller explicitly opted into raw content. A
/// scanner failure quarantines the field from the export: a content-free
/// diagnostic replaces the text, never the un-scanned content
/// (SECRET_SCANNING.md §6).
fn render_field(text: &str, include_secrets: bool) -> String {
    if include_secrets {
        text.to_string()
    } else {
        match secrets::scan(text) {
            Ok(findings) => secrets::redact(text, &findings),
            Err(_) => "«field unavailable: scan failed»".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::claude_code::ClaudeCodeAdapter;
    use crate::ingest::persist_session;
    use crate::storage::blob::BlobStore;

    fn persist(conn: &Connection, jsonl: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let parsed = ClaudeCodeAdapter::new().parse_str(jsonl, "e");
        persist_session(conn, "claude-code", "Claude Code", &parsed, &blobs).unwrap()
    }

    #[test]
    fn default_export_masks_secrets_override_includes_them() {
        let conn = crate::storage::open_in_memory().unwrap();
        let secret = format!("ghp{}", "_0123456789abcdefghijklmnopqrstuvwxyz");
        let jsonl = format!(
            "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{{\"role\":\"user\",\"content\":\"deploy with {secret} now\"}}}}\n"
        );
        let sid = persist(&conn, &jsonl);

        let masked = export_session_markdown(&conn, &sid, false)
            .unwrap()
            .unwrap();
        assert!(masked.contains("deploy with"));
        assert!(
            !masked.contains(&secret),
            "default export must mask the secret"
        );
        assert!(masked.contains("«redacted:github-token»"));

        let full = export_session_markdown(&conn, &sid, true).unwrap().unwrap();
        assert!(
            full.contains(&secret),
            "explicit override includes the secret"
        );
        assert!(full.contains("includes flagged secrets"));
    }

    #[test]
    fn opaque_content_is_omitted_and_unknown_session_is_none() {
        let conn = crate::storage::open_in_memory().unwrap();
        let jsonl = "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n";
        let sid = persist(&conn, jsonl);
        let md = export_session_markdown(&conn, &sid, false)
            .unwrap()
            .unwrap();
        assert!(md.starts_with("# "));
        assert!(export_session_markdown(&conn, "nope", false)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_scan_failure_quarantines_field_content_from_export() {
        let secret = format!("ghp{}", "_0123456789abcdefghijklmnopqrstuvwxyz");
        let text = format!("deploy with {secret} now");

        crate::secrets::set_fail_scans_for_test(true);
        let rendered = render_field(&text, false);
        crate::secrets::set_fail_scans_for_test(false);

        assert!(
            !rendered.contains(&secret),
            "un-scanned content must not reach an export"
        );
        assert!(
            rendered.contains("unavailable"),
            "a content-free diagnostic is shown instead"
        );
    }

    #[test]
    fn export_renders_structured_json_parts_when_text_is_absent() {
        let conn = crate::storage::open_in_memory().unwrap();
        let jsonl = concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":\"run command\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test\"}}]}}\n"
        );
        let sid = persist(&conn, jsonl);
        let md = export_session_markdown(&conn, &sid, false)
            .unwrap()
            .unwrap();
        assert!(md.contains("```json"));
        assert!(md.contains("cargo test"));
    }

    #[test]
    fn export_sanitizes_title_newlines_and_file_path_backticks() {
        let conn = crate::storage::open_in_memory().unwrap();
        let jsonl = concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":\"edit `file.rs`\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Edit\",\"input\":{\"file_path\":\"src/`malicious`.rs\"}}]}}\n"
        );
        let sid = persist(&conn, jsonl);
        let md = export_session_markdown(&conn, &sid, false)
            .unwrap()
            .unwrap();
        assert!(!md.contains("`src/`malicious`.rs`"));
        assert!(md.contains("`src/'malicious'.rs`"));
    }

    #[test]
    fn export_uses_four_backticks_when_json_contains_code_fence() {
        let conn = crate::storage::open_in_memory().unwrap();
        let jsonl = concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":\"explain\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Bash\",\"input\":{\"command\":\"echo ```nested```\"}}]}}\n"
        );
        let sid = persist(&conn, jsonl);
        let md = export_session_markdown(&conn, &sid, false)
            .unwrap()
            .unwrap();
        assert!(md.contains("````json\n"));
        assert!(md.contains("echo ```nested```"));
        assert!(md.contains("\n````\n"));
    }
}
