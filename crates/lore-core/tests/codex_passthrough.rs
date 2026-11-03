//! Codex tool payloads carry `internal_chat_message_metadata_passthrough`, a
//! large object of internal metadata that nothing in Lore reads. The archive
//! keeps the rest of the payload and drops it.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::adapters::codex::CodexAdapter;
use serde_json::{json, Value};

#[test]
fn codex_tool_parts_drop_the_metadata_passthrough() {
    let passthrough = json!({ "turn": { "trace": "internal-trace" } });
    let lines = [
        json!({"type": "session_meta", "timestamp": "2026-10-01T10:00:00.000Z", "payload": {
            "id": "019e0000-0000-7000-8000-000000000001", "cwd": "/proj",
            "cli_version": "0.133.0", "model_provider": "openai"}}),
        json!({"type": "response_item", "timestamp": "2026-10-01T10:00:01.000Z", "payload": {
            "type": "function_call", "name": "shell", "call_id": "call_1",
            "arguments": "{\"command\":[\"ls\"]}",
            "internal_chat_message_metadata_passthrough": passthrough}}),
        json!({"type": "response_item", "timestamp": "2026-10-01T10:00:02.000Z", "payload": {
            "type": "function_call_output", "call_id": "call_1",
            "output": {"output": "README.md\n", "metadata": {
                "exit_code": 0, "internal_chat_message_metadata_passthrough": passthrough}}}}),
    ];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let parsed = CodexAdapter::new().parse_str(&body, "k");
    let texts: Vec<&str> = parsed
        .messages
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|p| p.text.as_deref())
        .collect();
    assert_eq!(
        texts,
        [r#"{"metadata":{"exit_code":0},"output":"README.md\n"}"#]
    );
    let stored: Vec<Value> = parsed
        .messages
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|p| p.content_json.as_deref())
        .map(|j| serde_json::from_str(j).unwrap())
        .collect();
    assert_eq!(
        stored,
        [
            json!({"type": "function_call", "name": "shell", "call_id": "call_1",
                "arguments": "{\"command\":[\"ls\"]}"}),
            json!({"type": "function_call_output", "call_id": "call_1",
                "output": {"output": "README.md\n", "metadata": {"exit_code": 0}}}),
        ]
    );
}
