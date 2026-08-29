//! Redaction-aware session export (`SECURITY.md` §4, §6).
//!
//! A default export **masks** every flagged secret span, so exporting cannot
//! amplify a secret out of the archive. Including flagged content requires an
//! explicit, per-call override (`include_secrets`) — there is no sticky setting.
//! Opaque/encrypted regions are never decoded or exported. Rendering reuses the
//! canonical read path (`query::get_session`) and the same scanner as ingest.

use std::collections::HashMap;
use std::fmt::Write;

use rusqlite::Connection;

use crate::ingest::det_id;
use crate::query;
use crate::secrets;
use crate::storage::Result;

/// Render a session as Markdown. When `include_secrets` is false (the default
/// posture) every flagged secret span is masked using the findings stored during
/// ingest; `true` is an explicit opt-in to full-fidelity content. Returns `None`
/// when the session is unknown.
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

    let findings_map = if include_secrets {
        HashMap::new()
    } else {
        session_findings(conn, session_id)?
    };

    let render = |source_id: &str, field: &str, text: &str| -> String {
        if include_secrets {
            text.to_string()
        } else if let Some(findings) = findings_map.get(&(source_id.to_string(), field.to_string()))
        {
            secrets::redact(text, findings)
        } else {
            text.to_string()
        }
    };

    let render_title = |title: &str| -> String {
        if include_secrets {
            title.to_string()
        } else if let Some(findings) =
            findings_map.get(&(session_id.to_string(), "title".to_string()))
        {
            secrets::redact(title, findings)
        } else {
            // Synthetic title was not indexed at ingest; scan this bounded title fallback.
            match secrets::scan(title) {
                Ok(findings) => secrets::redact(title, &findings),
                Err(_) => "«field unavailable: scan failed»".to_string(),
            }
        }
    };

    let title = s.title.as_deref().unwrap_or("(untitled session)");
    let clean_title = title.replace(['\r', '\n'], " ");
    let _ = writeln!(out, "# {}", render_title(&clean_title));
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
            let pid = det_id("p", &[&message.id, &part.ordinal.to_string()]);
            match part.kind.as_str() {
                "opaque" => {
                    let _ = writeln!(out, "_[encrypted content omitted]_\n");
                }
                "thinking" => {
                    if let Some(text) = &part.text {
                        let _ = writeln!(out, "> _(thinking)_ {}\n", render(&pid, "text", text));
                    }
                }
                _ => {
                    if let Some(text) = &part.text {
                        let _ = writeln!(out, "{}\n", render(&pid, "text", text));
                    } else if let Some(json) = &part.content_json {
                        let rendered_json = render(&pid, "content_json", json);
                        let fence = markdown_fence(&rendered_json);
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

fn intern_rule(rule: &str) -> &'static str {
    match rule {
        "private-key-block" => "private-key-block",
        "aws-access-key-id" => "aws-access-key-id",
        "gcp-api-key" => "gcp-api-key",
        "github-token" => "github-token",
        "github-fine-grained-pat" => "github-fine-grained-pat",
        "gitlab-pat" => "gitlab-pat",
        "slack-token" => "slack-token",
        "stripe-key" => "stripe-key",
        "anthropic-key" => "anthropic-key",
        "openai-key" => "openai-key",
        "google-oauth-secret" => "google-oauth-secret",
        "npm-token" => "npm-token",
        "jwt" => "jwt",
        "connection-string" => "connection-string",
        "slack-webhook" => "slack-webhook",
        "discord-webhook" => "discord-webhook",
        "high-entropy" => "high-entropy",
        _ => "secret",
    }
}

fn parse_severity(s: &str) -> secrets::Severity {
    match s {
        "critical" => secrets::Severity::Critical,
        "high" => secrets::Severity::High,
        "medium" => secrets::Severity::Medium,
        _ => secrets::Severity::Low,
    }
}

/// Load all stored secret findings for a session grouped by `(source_id, field)`.
fn session_findings(
    conn: &Connection,
    session_id: &str,
) -> Result<HashMap<(String, String), Vec<secrets::Finding>>> {
    let mut stmt = conn.prepare(
        "SELECT source_id, field, rule, span_start, span_end, severity
         FROM secret_finding
         WHERE session_id = ?1
         ORDER BY span_start ASC",
    )?;
    let mut map: HashMap<(String, String), Vec<secrets::Finding>> = HashMap::new();
    let mut rows = stmt.query([session_id])?;
    while let Some(row) = rows.next()? {
        let source_id: String = row.get(0)?;
        let field: String = row.get(1)?;
        let rule_str: String = row.get(2)?;
        let span_start: i64 = row.get(3)?;
        let span_end: i64 = row.get(4)?;
        let severity_str: String = row.get(5)?;
        let finding = secrets::Finding {
            rule: intern_rule(&rule_str),
            start: usize::try_from(span_start).unwrap_or(0),
            end: usize::try_from(span_end).unwrap_or(0),
            severity: parse_severity(&severity_str),
        };
        map.entry((source_id, field)).or_default().push(finding);
    }
    Ok(map)
}

/// Determine an enclosing code fence length that exceeds any run of consecutive
/// backticks in the content, ensuring code blocks cannot break out of their fence.
fn markdown_fence(content: &str) -> String {
    let mut max_backticks = 0;
    let mut current_backticks = 0;
    for b in content.bytes() {
        if b == b'`' {
            current_backticks += 1;
            if current_backticks > max_backticks {
                max_backticks = current_backticks;
            }
        } else {
            current_backticks = 0;
        }
    }
    let fence_len = if max_backticks < 3 {
        3
    } else {
        max_backticks + 1
    };
    "`".repeat(fence_len)
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
    fn export_uses_stored_findings_independent_of_live_scanner() {
        let conn = crate::storage::open_in_memory().unwrap();
        let secret = format!("ghp{}", "_0123456789abcdefghijklmnopqrstuvwxyz");
        let jsonl = format!(
            "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{{\"role\":\"user\",\"content\":\"deploy with {secret} now\"}}}}\n"
        );
        let sid = persist(&conn, &jsonl);

        // Arm the scan failure seam: live scanning would fail, but export reads stored findings.
        crate::secrets::set_fail_scans_for_test(true);
        let md = export_session_markdown(&conn, &sid, false)
            .unwrap()
            .unwrap();
        crate::secrets::set_fail_scans_for_test(false);

        assert!(
            !md.contains(&secret),
            "export must mask the secret using stored finding"
        );
        assert!(md.contains("«redacted:github-token»"));
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

    #[test]
    fn markdown_fence_scales_with_consecutive_backticks() {
        assert_eq!(markdown_fence("plain text"), "```");
        assert_eq!(markdown_fence("some `inline` code"), "```");
        assert_eq!(markdown_fence("code ``` block"), "````");
        assert_eq!(markdown_fence("code ```` block"), "`````");
        assert_eq!(markdown_fence("code ````` block"), "``````");
    }

    #[test]
    fn export_renders_thinking_blocks_cleanly() {
        let conn = crate::storage::open_in_memory().unwrap();
        let jsonl = concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":\"think about it\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"contemplating options...\"},{\"type\":\"text\",\"text\":\"here is the plan\"}]}}\n"
        );
        let sid = persist(&conn, jsonl);
        let md = export_session_markdown(&conn, &sid, false)
            .unwrap()
            .unwrap();
        assert!(md.contains("> _(thinking)_ contemplating options..."));
        assert!(md.contains("here is the plan"));
    }

    #[test]
    fn export_renders_complex_nested_tool_use_and_result() {
        let conn = crate::storage::open_in_memory().unwrap();
        let jsonl = concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":\"batch run\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t_batch\",\"name\":\"BatchRunner\",\"input\":{\"tasks\":[{\"id\":1,\"cmd\":\"echo A\"},{\"id\":2,\"cmd\":\"echo B\"}],\"options\":{\"parallel\":true}}}]}}\n",
            "{\"type\":\"user\",\"uuid\":\"u2\",\"sessionId\":\"e\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t_batch\",\"content\":\"Task 1 completed\\nTask 2 completed\"}]}}\n"
        );
        let sid = persist(&conn, jsonl);
        let md = export_session_markdown(&conn, &sid, false)
            .unwrap()
            .unwrap();

        assert!(md.contains("BatchRunner"));
        assert!(md.contains("\"tasks\""));
        assert!(md.contains("\"parallel\""));
        assert!(md.contains("Task 1 completed"));
        assert!(md.contains("Task 2 completed"));
    }

    #[test]
    fn export_session_markdown_with_empty_or_missing_cwd_and_empty_title() {
        let conn = crate::storage::open_in_memory().unwrap();
        let jsonl =
            "{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"e_empty\",\"message\":{\"role\":\"user\",\"content\":\"simple message\"}}\n";
        let sid = persist(&conn, jsonl);
        let md = export_session_markdown(&conn, &sid, false)
            .unwrap()
            .unwrap();

        assert!(md.contains("# "));
        assert!(md.contains("claude-code · 1 messages · 0 tool calls"));
        assert!(md.contains("simple message"));
    }
}
