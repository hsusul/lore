//! I4 acceptance: fixtures are version-keyed (`fixtures/VERSIONS.json`), and a
//! round-trip drift validator surfaces keys a fixture carries that the adapter
//! does not (yet) read — a **warning**, never a hard failure, consistent with
//! tolerant parsing (`AGENT_ADAPTERS.md` §5 rule 1: an unknown field degrades to
//! `partial`, never a crash).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::codex::CodexAdapter;
use lore_core::adapters::AgentAdapter;
use serde_json::Value;

/// Keys the Claude Code adapter reads across its schema surface (event envelope
/// plus the fields it extracts). Kept in sync with `docs/agents/CLAUDE_CODE.md`;
/// a fixture key outside this set is the drift signal.
const CLAUDE_KEYS: &[&str] = &[
    "type",
    "uuid",
    "parentUuid",
    "sessionId",
    "timestamp",
    "cwd",
    "gitBranch",
    "version",
    "isSidechain",
    "userType",
    "message",
    "requestId",
    "customTitle",
    "role",
    "content",
    "text",
    "thinking",
    "signature",
    "model",
    "stop_reason",
    "usage",
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "id",
    "name",
    "input",
    "is_error",
    "file_path",
    "old_string",
    "new_string",
    "tool_use_id",
    "attachment",
    "notebook_path",
    "path",
];

/// Keys the Codex adapter reads (rollout envelope plus payload fields).
const CODEX_KEYS: &[&str] = &[
    "type",
    "timestamp",
    "payload",
    "id",
    "cwd",
    "cli_version",
    "model_provider",
    "model",
    "role",
    "content",
    "text",
    "name",
    "arguments",
    "call_id",
    "output",
    "status",
    "source",
    "git",
    "branch",
    "commit_hash",
    "repository_url",
    "effort",
    "turn_id",
    "summary",
    "last_agent_message",
    "duration_ms",
    "success",
    "changes",
    "unified_diff",
    "move_path",
    "info",
    "last_token_usage",
    "input_tokens",
    "output_tokens",
    "cached_input_tokens",
    "cache_write_input_tokens",
    "reasoning_output_tokens",
    "total_tokens",
    "model_context_window",
    "total_token_usage",
    "rate_limits",
    "encrypted_content",
];

/// Recursively collect every object key (schema keys only in practice; file-path
/// keys in `changes` arrays are data, not schema, and are excluded by scoping the
/// scan to the version-keyed fixtures below).
fn collect_keys(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                out.insert(key.clone());
                collect_keys(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_keys(item, out);
            }
        }
        _ => {}
    }
}

fn known_keys(agent: &str) -> BTreeSet<String> {
    let keys = if agent == "claude_code" {
        CLAUDE_KEYS
    } else {
        CODEX_KEYS
    };
    keys.iter().map(|k| (*k).to_string()).collect()
}

/// Keys present in `line` but not in `known`. Empty (or an unparseable line)
/// returns an empty list.
fn unknown_keys(line: &str, known: &BTreeSet<String>) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let mut present = BTreeSet::new();
    collect_keys(&value, &mut present);
    let mut unknown: Vec<String> = present.difference(known).cloned().collect();
    unknown.sort();
    unknown
}

fn fixture(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(rel);
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn version_keyed_fixtures_parse_and_drift_is_warned_not_failed() {
    // The version-keyed fixtures each represent a specific agent version. The
    // hard assertion is that they parse into messages (no panic, no hard fail);
    // unknown keys are surfaced as eprintln warnings only.
    check_fixture(
        "claude_code",
        "v2.1.0_sample.jsonl",
        &ClaudeCodeAdapter::new(),
    );
    check_fixture("codex", "v0.136.0_sample.jsonl", &CodexAdapter::new());
}

fn check_fixture(agent: &str, name: &str, adapter: &impl AgentAdapter) {
    let content = fixture(&format!("{agent}/{name}"));
    let parsed = adapter.parse_content(&content, "drift");
    assert!(
        !parsed.messages.is_empty(),
        "{agent}/{name} must parse to at least one message"
    );
    let known = known_keys(agent);
    for (i, line) in content.lines().enumerate() {
        let unknown = unknown_keys(line, &known);
        if !unknown.is_empty() {
            eprintln!("WARN {agent}/{name}:{i}: unknown keys: {unknown:?}");
        }
    }
}

#[test]
fn unknown_keys_flags_a_field_outside_the_known_surface() {
    let known = known_keys("claude_code");
    let unknown = unknown_keys(r#"{"type":"user","mode":"plan"}"#, &known);
    assert!(
        unknown.contains(&"mode".to_string()),
        "an unseen field must be flagged: {unknown:?}"
    );
    // Known fields are not flagged.
    assert!(unknown_keys(r#"{"type":"user","uuid":"u"}"#, &known).is_empty());
}
