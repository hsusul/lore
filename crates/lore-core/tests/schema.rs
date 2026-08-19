//! V0 schema (migration 0002) verification: tables exist, foreign keys and
//! uniqueness (incl. partial-unique NULL semantics) are enforced, and the
//! external-content FTS5 index tracks `search_document`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rusqlite::params;

fn db() -> rusqlite::Connection {
    lore_core::storage::open_in_memory().unwrap()
}

#[test]
fn all_v0_tables_exist() {
    let conn = db();
    let expected = [
        "agent",
        "blob",
        "source_artifact",
        "source_artifact_path",
        "ingest_state",
        "repository",
        "repository_identity_evidence",
        "worktree",
        "agent_session",
        "session_source",
        "session_segment",
        "message",
        "message_part",
        "tool_call",
        "file_event",
        "git_observation",
        "secret_finding",
        "search_document",
        "search_fts",
        // infra from 0001
        "setting",
        "job",
        // organizational metadata from 0006
        "folder",
        "session_folder",
    ];
    for t in expected {
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = ?1 AND type IN ('table','view')",
                [t],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "missing table/vtable: {t}");
    }
}

#[test]
fn foreign_keys_are_enforced() {
    let conn = db();
    // Inserting a session for a non-existent agent must fail.
    let res = conn.execute(
        "INSERT INTO agent_session (id, agent_id, dedupe_key) VALUES ('s1', 'ghost', 'k1')",
        [],
    );
    assert!(res.is_err(), "FK to missing agent must be rejected");
}

#[test]
fn data_model_indexes_exist() {
    // DATA_MODEL.md §8 declares these as performance-critical; they are created
    // by migrations 0004, 0005, and 0009. Assert the documented contract holds.
    let conn = db();
    let expected = [
        ("ix_repo_identity_kind_hash", "repository_identity_evidence"),
        ("ix_worktree_repository_path", "worktree"),
        ("ix_source_artifact_agent_path", "source_artifact"),
        ("ix_source_artifact_agent_native_hash", "source_artifact"),
        ("ix_session_folder_folder", "session_folder"),
        ("ix_secret_session", "secret_finding"),
        ("ix_session_started", "agent_session"),
        ("ix_session_source_artifact", "session_source"),
    ];
    for (idx, tbl) in expected {
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE name = ?1 AND type = 'index' AND tbl_name = ?2",
                [idx, tbl],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "missing documented index: {idx} on {tbl}");
    }
}

fn seed_session(conn: &rusqlite::Connection) {
    conn.execute(
        "INSERT INTO agent (id, display_name, detected) VALUES ('claude-code','Claude Code',1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO agent_session (id, agent_id, native_session_id, dedupe_key)
         VALUES ('s1','claude-code','nat-1','d1')",
        [],
    )
    .unwrap();
}

#[test]
fn tool_call_native_id_is_unique_per_session() {
    let conn = db();
    seed_session(&conn);
    // Minimal message + part to satisfy tool_call FKs.
    conn.execute(
        "INSERT INTO message (id, session_id, seq, role) VALUES ('m1','s1',0,'assistant')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message_part (id, message_id, ordinal, kind) VALUES ('p1','m1',0,'tool_use')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tool_call (id, session_id, call_part_id, native_call_id, name)
         VALUES ('t1','s1','p1','call_1','Edit')",
        [],
    )
    .unwrap();
    let dup = conn.execute(
        "INSERT INTO tool_call (id, session_id, call_part_id, native_call_id, name)
         VALUES ('t2','s1','p1','call_1','Edit')",
        [],
    );
    assert!(
        dup.is_err(),
        "duplicate native_call_id in a session must fail"
    );
}

#[test]
fn session_uniqueness_null_semantics() {
    let conn = db();
    conn.execute(
        "INSERT INTO agent (id, display_name, detected) VALUES ('codex','Codex',1)",
        [],
    )
    .unwrap();
    // Two sessions with NULL native id but the SAME dedupe_key must collide.
    conn.execute(
        "INSERT INTO agent_session (id, agent_id, native_session_id, dedupe_key)
         VALUES ('a','codex',NULL,'same')",
        [],
    )
    .unwrap();
    let collide = conn.execute(
        "INSERT INTO agent_session (id, agent_id, native_session_id, dedupe_key)
         VALUES ('b','codex',NULL,'same')",
        [],
    );
    assert!(
        collide.is_err(),
        "dedupe_key must be unique when native id is NULL"
    );

    // Two sessions with the SAME native id must collide.
    conn.execute(
        "INSERT INTO agent_session (id, agent_id, native_session_id, dedupe_key)
         VALUES ('c','codex','nat','d-c')",
        [],
    )
    .unwrap();
    let collide2 = conn.execute(
        "INSERT INTO agent_session (id, agent_id, native_session_id, dedupe_key)
         VALUES ('d','codex','nat','d-d')",
        [],
    );
    assert!(
        collide2.is_err(),
        "native_session_id must be unique per agent"
    );
}

#[test]
fn fts_tracks_search_document() {
    let conn = db();
    seed_session(&conn);
    conn.execute(
        "INSERT INTO search_document
            (session_id, source_kind, source_id, field, redacted_text, created_at)
         VALUES ('s1','message_part','p1','text','fix the stripe webhook signature',1)",
        params![],
    )
    .unwrap();
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM search_fts WHERE search_fts MATCH 'webhook'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 1, "inserting a search_document must index it in FTS5");

    // Deleting the document must remove it from the FTS index too.
    conn.execute("DELETE FROM search_document WHERE source_id = 'p1'", [])
        .unwrap();
    let after: i64 = conn
        .query_row(
            "SELECT count(*) FROM search_fts WHERE search_fts MATCH 'webhook'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after, 0, "deleting a search_document must de-index it");
}
