//! A session a program started is marked automated in the archive, and so is
//! one a person started and a program resumed (`claude -p --resume`), even
//! when only the appended lines say so.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::ingest::{ingest_file, IngestOutcome};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

const SESSION: &str = "16161616-0000-4000-8000-000000000001";

fn line(n: usize, role: &str, entrypoint: &str) -> String {
    let message = if role == "user" {
        serde_json::json!({"role": "user", "content": format!("turn {n}")})
    } else {
        serde_json::json!({"role": "assistant", "model": "claude-x",
            "content": [{"type": "text", "text": format!("reply {n}")}]})
    };
    let line = serde_json::json!({
        "type": role, "uuid": format!("u{n}"),
        "parentUuid": (n > 0).then(|| format!("u{}", n - 1)),
        "sessionId": SESSION, "timestamp": format!("2026-10-31T18:00:{n:02}.000Z"),
        "cwd": "/nonexistent/lore-test", "version": "2.1.0", "entrypoint": entrypoint,
        "isSidechain": false, "userType": "external", "message": message,
    });
    format!("{line}\n")
}

fn metadata(conn: &Connection) -> Option<String> {
    conn.query_row("SELECT metadata_json FROM agent_session", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn print_mode_sessions_are_marked_and_desktop_ones_are_not() {
    for (entrypoint, want) in [
        ("sdk-cli", Some(r#"{"automated":true}"#)),
        ("claude-desktop", None),
        ("cli", None),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let conn = lore_core::storage::open(&dir.path().join("lore.db")).unwrap();
        let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
        let path = dir.path().join(format!("{SESSION}.jsonl"));
        std::fs::write(
            &path,
            line(0, "user", entrypoint) + &line(1, "assistant", entrypoint),
        )
        .unwrap();
        ingest_file(&conn, &ClaudeCodeAdapter::new(), &path, &blobs).unwrap();
        assert_eq!(metadata(&conn).as_deref(), want, "{entrypoint}");
    }
}

#[test]
fn a_session_a_program_resumed_is_marked_on_append() {
    let dir = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open(&dir.path().join("lore.db")).unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let path = dir.path().join(format!("{SESSION}.jsonl"));
    std::fs::write(
        &path,
        line(0, "user", "claude-desktop") + &line(1, "assistant", "claude-desktop"),
    )
    .unwrap();
    ingest_file(&conn, &ClaudeCodeAdapter::new(), &path, &blobs).unwrap();
    assert_eq!(metadata(&conn), None);

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all((line(2, "user", "sdk-cli") + &line(3, "assistant", "sdk-cli")).as_bytes())
        .unwrap();
    let outcome = ingest_file(&conn, &ClaudeCodeAdapter::new(), &path, &blobs).unwrap();
    assert!(
        matches!(outcome, IngestOutcome::Ingested { .. }),
        "{outcome:?}"
    );
    assert_eq!(metadata(&conn).as_deref(), Some(r#"{"automated":true}"#));

    // A person picking it up again does not clear the mark.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(line(4, "user", "claude-desktop").as_bytes())
        .unwrap();
    ingest_file(&conn, &ClaudeCodeAdapter::new(), &path, &blobs).unwrap();
    assert_eq!(metadata(&conn).as_deref(), Some(r#"{"automated":true}"#));
}
