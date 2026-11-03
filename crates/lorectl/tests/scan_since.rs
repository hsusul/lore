//! `lorectl scan --since <DURATION>` ingests only the sources modified in that
//! window, so a first run over a large history can start with recent work.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

fn lorectl(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .env("HOME", home)
        .env("LORE_ARCHIVE_DIR", home.join("archive"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .output()
        .unwrap()
}

/// A Codex rollout for session `n`, its file last modified `days_ago`.
fn rollout(home: &Path, n: usize, days_ago: u64) {
    let day = home.join("codex/sessions/2026/08/01");
    std::fs::create_dir_all(&day).unwrap();
    let path = day.join(format!("rollout-{n:012x}.jsonl"));
    let lines = [
        serde_json::json!({"type": "session_meta", "timestamp": "2026-08-01T10:00:00.000Z",
            "payload": {"id": format!("019e0000-0000-7000-8000-{n:012x}"), "cwd": "/proj",
                "cli_version": "0.133.0", "model_provider": "openai"}}),
        serde_json::json!({"type": "response_item", "timestamp": "2026-08-01T10:00:01.000Z",
            "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": format!("session {n}")}]}}),
    ];
    std::fs::write(&path, lines.map(|l| format!("{l}\n")).concat()).unwrap();
    let when = SystemTime::now() - Duration::from_secs(days_ago * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

fn sessions(home: &Path) -> i64 {
    let conn = lore_core::storage::open_read_only(&home.join("archive/lore.db")).unwrap();
    conn.query_row("SELECT COUNT(*) FROM agent_session", [], |r| r.get(0))
        .unwrap()
}

fn stdout(o: &Output) -> String {
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn scan_since_ingests_only_recent_sources_and_a_full_scan_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    rollout(home, 1, 2);
    rollout(home, 2, 3);
    rollout(home, 3, 90);

    let out = stdout(&lorectl(home, &["scan", "--since", "30d"]));
    assert!(out.contains("ingested 2  skipped 0  failed 0"), "{out}");
    assert!(
        out.contains("only sources modified in the last 30d"),
        "{out}"
    );
    assert_eq!(sessions(home), 2);

    let out = stdout(&lorectl(home, &["scan"]));
    assert!(out.contains("ingested 1  skipped 0  failed 0"), "{out}");
    assert_eq!(sessions(home), 3);
}

#[test]
fn scan_since_never_marks_an_older_source_missing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    rollout(home, 1, 2);
    rollout(home, 2, 90);
    stdout(&lorectl(home, &["scan"]));
    stdout(&lorectl(home, &["scan", "--since", "7d"]));
    let conn = lore_core::storage::open_read_only(&home.join("archive/lore.db")).unwrap();
    let states: Vec<String> = conn
        .prepare("SELECT state FROM source_artifact ORDER BY current_path")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(states.len(), 2);
    assert!(states.iter().all(|s| s != "missing"), "{states:?}");
}

#[test]
fn scan_since_needs_a_number_and_a_unit() {
    let dir = tempfile::tempdir().unwrap();
    for bad in ["30", "d", "-3d", "30w", "1.5d"] {
        let out = lorectl(dir.path(), &["scan", "--since", bad]);
        assert_eq!(out.status.code(), Some(1), "{bad}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("like 30d, 12h, or 90m"),
            "{bad}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = lorectl(dir.path(), &["status", "--since", "30d"]);
    assert_eq!(out.status.code(), Some(1));
}
