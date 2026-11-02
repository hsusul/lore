//! `query::find_session` resolves an id an agent may hold, either Lore's
//! archive session id or the agent's own native session id, to the archived
//! session and the agent that recorded it.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::codex::CodexAdapter;
use lore_core::adapters::AgentAdapter;
use lore_core::ingest::{ingest_file, IngestOutcome};
use lore_core::query::{find_session, SessionOwner};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

const CLAUDE_NATIVE: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const CODEX_NATIVE: &str = "019e0000-0000-7000-8000-000000000001";

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(rel)
}

fn ingest(conn: &Connection, adapter: &dyn AgentAdapter, rel: &str, blobs: &BlobStore) -> String {
    match ingest_file(conn, adapter, &fixture(rel), blobs).unwrap() {
        IngestOutcome::Ingested { session_id, .. } => Some(session_id),
        IngestOutcome::Skipped => None,
    }
    .expect("fixture was ingested, not skipped")
}

fn owner(archive_id: &str, agent_id: &str) -> Option<SessionOwner> {
    Some(SessionOwner {
        archive_id: archive_id.to_string(),
        agent_id: agent_id.to_string(),
    })
}

#[test]
fn archive_and_native_ids_resolve_to_their_own_session_and_agent() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();
    let claude = ingest(
        &conn,
        &ClaudeCodeAdapter::new(),
        "claude_code/basic_text.jsonl",
        &blobs,
    );
    let codex = ingest(&conn, &CodexAdapter::new(), "codex/minimal.jsonl", &blobs);

    // Archive ids are deterministic, so they are pinned literally here.
    assert_eq!(claude, "s_0cb00a9167e78bc7");
    assert_eq!(codex, "s_eda444c63f132f60");

    let claude_owner = owner(&claude, "claude-code");
    let codex_owner = owner(&codex, "codex");

    // The archive id gives its owner.
    assert_eq!(find_session(&conn, &claude).unwrap(), claude_owner);
    assert_eq!(find_session(&conn, &codex).unwrap(), codex_owner);

    // The native id gives the same archive id and agent. Each agent's ids
    // resolve only to that agent's session, so the two never cross.
    assert_eq!(find_session(&conn, CLAUDE_NATIVE).unwrap(), claude_owner);
    assert_eq!(find_session(&conn, CODEX_NATIVE).unwrap(), codex_owner);

    // Unknown ids give None.
    assert_eq!(find_session(&conn, "s_does_not_exist").unwrap(), None);
    assert_eq!(
        find_session(&conn, "bbbbbbbb-0000-4000-8000-000000000009").unwrap(),
        None
    );
    assert_eq!(find_session(&conn, "").unwrap(), None);
}
