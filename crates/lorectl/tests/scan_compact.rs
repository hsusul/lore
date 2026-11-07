//! `lorectl scan` compacts the archive after a successful scan, as the desktop
//! worker does: attachment bodies stored once, tool outputs stored once, and
//! the one `VACUUM` into incremental mode, after a backup. `--no-compact`
//! skips it.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

use lore_core::compact;

fn lorectl(home: &Path, args: &[&str]) -> String {
    let out: Output = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .env("HOME", home)
        .env("LORE_ARCHIVE_DIR", home.join("archive"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A Claude Code session with two attachment events and one tool result
/// whose payload repeats its output.
fn transcript(home: &Path) {
    let project = home.join("claude/projects/-proj");
    std::fs::create_dir_all(&project).unwrap();
    let session = "5e550000-0000-4000-8000-000000000001";
    let base = |i: u32, kind: &str| {
        serde_json::json!({
            "type": kind, "uuid": format!("{kind}-{i}"), "sessionId": session,
            "cwd": "/proj", "timestamp": format!("2026-10-01T10:00:0{i}.000Z"),
        })
    };
    let mut lines = Vec::new();
    let mut prompt = base(0, "user");
    prompt["message"] = serde_json::json!({"role": "user", "content": "list the files"});
    lines.push(prompt);
    for (i, body) in [
        (1, "injected context, the same in many sessions"),
        (2, "a second snapshot of context"),
    ] {
        let mut event = base(i, "attachment");
        event["attachment"] = serde_json::json!({"type": "prompt_snapshot", "content": body});
        lines.push(event);
    }
    let mut call = base(3, "assistant");
    call["message"] = serde_json::json!({"role": "assistant", "content": [
        {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}]});
    lines.push(call);
    let mut result = base(4, "user");
    result["message"] = serde_json::json!({"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "t1",
         "content": "README.md\nsrc/main.rs\nCargo.toml\nan output long enough to slim\n"}]});
    lines.push(result);
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(project.join(format!("{session}.jsonl")), text).unwrap();
}

fn open(home: &Path) -> rusqlite::Connection {
    lore_core::storage::open(&home.join("archive/lore.db")).unwrap()
}

/// Put the archive in the shape one ingested before stored-once bodies has:
/// attachment bodies and tool payloads inline, and no incremental vacuum.
fn make_legacy(home: &Path) {
    let conn = open(home);
    conn.execute_batch(
        "UPDATE message_part
            SET content_json = (SELECT v.content_json FROM message_part_full v
                                 WHERE v.id = message_part.id)
          WHERE kind = 'tool_result' AND instr(content_json, char(1)) > 0;
         UPDATE message_part
            SET content_json = (SELECT body FROM part_body WHERE hash = message_part.body_hash),
                body_hash = NULL
          WHERE body_hash IS NOT NULL;
         DELETE FROM part_body;
         INSERT OR REPLACE INTO setting (key, value_json, updated_at)
            SELECT 'compact.tool_output_until', CAST(max(rowid) AS TEXT), 0 FROM message_part;
         DELETE FROM setting WHERE key IN ('compact.tool_output_after',
                                           'compact.tool_output_failed_rows');
         PRAGMA auto_vacuum = NONE; VACUUM;",
    )
    .unwrap();
    assert_eq!(compact::inline_attachments(&conn).unwrap(), 2);
    assert!(compact::tool_outputs_pending(&conn).unwrap());
    assert!(!compact::is_incremental(&conn).unwrap());
}

/// Every part's body as readers see it.
fn view(home: &Path) -> Vec<(String, Option<String>)> {
    let conn = open(home);
    let mut stmt = conn
        .prepare("SELECT id, content_json FROM message_part_full ORDER BY id")
        .unwrap();
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

fn backups(home: &Path) -> usize {
    std::fs::read_dir(home.join("archive/backups")).map_or(0, Iterator::count)
}

#[test]
fn scan_compacts_an_old_archive_once_and_then_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    transcript(home);
    lorectl(home, &["scan"]);
    make_legacy(home);
    let before = view(home);

    let out = lorectl(home, &["scan"]);
    assert!(out.contains("ingested 0 "), "{out}");
    let line = out
        .lines()
        .find(|l| l.starts_with("compacted"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(line.starts_with("compacted 3 row(s), reclaimed "), "{out}");
    assert!(line.ends_with(" bytes"), "{out}");
    {
        let conn = open(home);
        assert_eq!(compact::inline_attachments(&conn).unwrap(), 0);
        assert!(!compact::tool_outputs_pending(&conn).unwrap());
        assert!(compact::is_incremental(&conn).unwrap());
        let vacuum = lore_core::settings::get(&conn, compact::VACUUM_SETTING).unwrap();
        assert_eq!(vacuum.as_deref(), Some("\"vacuumed\""));
        let error = lore_core::settings::get(&conn, compact::ERROR_SETTING).unwrap();
        assert_eq!(error.as_deref(), Some("null"));
    }
    assert_eq!(view(home), before, "lossless");
    assert_eq!(backups(home), 1, "backed up before the first batch");

    let again = lorectl(home, &["scan"]);
    assert!(!again.contains("compact"), "{again}");
    assert_eq!(view(home), before);
    assert_eq!(backups(home), 1);

    let json: serde_json::Value =
        serde_json::from_str(lorectl(home, &["scan", "--json"]).trim()).unwrap();
    assert_eq!(json["compact"]["converted"], 0, "{json}");
    assert_eq!(json["compact"]["reclaimed_bytes"], 0, "{json}");
}

#[test]
fn no_compact_leaves_an_old_archive_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    transcript(home);
    lorectl(home, &["scan"]);
    make_legacy(home);

    let out = lorectl(home, &["scan", "--no-compact"]);
    assert!(!out.contains("compact"), "{out}");
    let conn = open(home);
    assert_eq!(compact::inline_attachments(&conn).unwrap(), 2);
    assert!(compact::tool_outputs_pending(&conn).unwrap());
    assert!(!compact::is_incremental(&conn).unwrap());
    assert_eq!(backups(home), 0);
}
