//! A message the person types while Claude Code is busy is queued and handed
//! to the model mid-turn as a `queued_command` attachment. It is the person's
//! turn like any other, so a correction typed that way is mined, once, even
//! though the queue bookkeeping and a later compaction summary repeat it.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, Signal};
use serde_json::{json, Value};

const S: &str = "eeeeeeee-0000-4000-8000-0000000000a1";
const TYPED: &str = "don't touch the payments module, revert it";

fn line(kind: &str, uuid: &str, ts: &str, body: Value) -> Value {
    let mut l = json!({
        "type": kind, "uuid": uuid, "sessionId": S, "timestamp": ts,
        "cwd": "/repo/app", "version": "2.1.0", "isSidechain": false,
        "userType": "external",
    });
    if let (Some(l), Some(b)) = (l.as_object_mut(), body.as_object()) {
        l.extend(b.clone());
    }
    l
}

#[test]
fn a_queued_correction_is_mined_once() {
    let dir = tempfile::tempdir().unwrap();
    let lines = [
        line(
            "user",
            "u0",
            "2026-08-01T10:00:00.000Z",
            json!({"message": {"role": "user", "content": "finish the checkout flow"}}),
        ),
        line(
            "assistant",
            "a1",
            "2026-08-01T10:00:01.000Z",
            json!({"message": {"role": "assistant", "model": "claude-x", "content": [
                {"type": "text", "text": "Renaming the payments module helpers first."},
                {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "git mv a b"}}
            ]}}),
        ),
        json!({"type": "queue-operation", "operation": "enqueue", "sessionId": S,
            "timestamp": "2026-08-01T10:00:02.000Z", "content": TYPED}),
        line(
            "user",
            "r1",
            "2026-08-01T10:00:03.000Z",
            json!({"message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}
            ]}}),
        ),
        line(
            "attachment",
            "q1",
            "2026-08-01T10:00:02.000Z",
            json!({"attachment": {"type": "queued_command", "prompt": TYPED,
                "source_uuid": "11111111-0000-4000-8000-000000000001",
                "delivery_id": "22222222-0000-4000-8000-000000000002",
                "commandMode": "prompt", "origin": {"kind": "human"},
                "timestamp": "2026-08-01T10:00:02.000Z", "humanTurn": true},
                "renderedRole": "system"}),
        ),
        json!({"type": "queue-operation", "operation": "remove", "sessionId": S,
            "timestamp": "2026-08-01T10:00:04.000Z", "content": TYPED,
            "reason": "absorbed_mid_turn"}),
        line(
            "assistant",
            "a2",
            "2026-08-01T10:00:05.000Z",
            json!({"message": {"role": "assistant", "model": "claude-x", "content": [
                {"type": "text", "text": "Reverted the payments module."}
            ]}}),
        ),
    ];
    let conn = archive::claude_raw_archive(dir.path(), S, &lines);

    let found = corrections(&conn).unwrap();
    let mined: Vec<(&str, Signal)> = found.iter().map(|c| (c.text.as_str(), c.signal)).collect();
    assert_eq!(mined, [(TYPED, Signal::Correction)]);
    assert_eq!(
        found[0].following.as_deref(),
        Some("Reverted the payments module.")
    );

    let stored: i64 = conn
        .query_row(
            "SELECT count(*) FROM search_document WHERE redacted_text = ?1",
            [TYPED],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, 1, "searchable once, not once per queue copy");
}
