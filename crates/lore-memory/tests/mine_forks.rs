//! A Claude Code resume or fork copies a conversation into a new session id,
//! user turns and timestamps included. A copy is the same conversation, so it
//! never counts as a second session toward a lesson.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, lessons};

const ORIGINAL: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const FORK: &str = "aaaaaaaa-0000-4000-8000-000000000002";
const LATER: &str = "aaaaaaaa-0000-4000-8000-000000000003";

/// The conversation both copies carry, then one turn only the fork has.
const SHARED: &[(&str, &str, &str)] = &[
    ("user", "add axios", "2026-10-01T10:00:00.000Z"),
    (
        "assistant",
        "Running npm install axios.",
        "2026-10-01T10:00:05.000Z",
    ),
    ("user", "no, use pnpm not npm", "2026-10-01T10:00:09.000Z"),
    ("assistant", "Switched to pnpm.", "2026-10-01T10:00:12.000Z"),
];

#[test]
fn a_forked_copy_of_one_conversation_is_not_a_second_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut fork = SHARED.to_vec();
    fork.push(("user", "now add a timeout", "2026-10-01T11:00:00.000Z"));
    let conn = archive::claude_archive(dir.path(), &[(ORIGINAL, SHARED), (FORK, &fork)]);
    assert_eq!(
        corrections(&conn).unwrap().len(),
        2,
        "both copies are archived"
    );
    let found = lessons(corrections(&conn).unwrap());
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].conversations, 1,
        "the copy is the same conversation"
    );
    assert_eq!(found[0].receipts.len(), 1);
}

#[test]
fn a_real_second_conversation_still_counts_once_beside_the_copies() {
    let dir = tempfile::tempdir().unwrap();
    let later: &[(&str, &str, &str)] = &[
        ("user", "add dayjs", "2026-10-05T09:00:00.000Z"),
        (
            "assistant",
            "npm install dayjs done.",
            "2026-10-05T09:00:04.000Z",
        ),
        (
            "user",
            "I told you, pnpm not npm",
            "2026-10-05T09:00:08.000Z",
        ),
    ];
    let conn = archive::claude_archive(
        dir.path(),
        &[(ORIGINAL, SHARED), (FORK, SHARED), (LATER, later)],
    );
    let found = lessons(corrections(&conn).unwrap());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].conversations, 2);
    let sessions: Vec<&str> = found[0]
        .receipts
        .iter()
        .map(|r| r.session.as_str())
        .collect();
    let later_id = archive::claude_session_id(&conn, LATER);
    assert_eq!(sessions[0], later_id);
    let original_or_fork = [
        archive::claude_session_id(&conn, ORIGINAL),
        archive::claude_session_id(&conn, FORK),
    ];
    assert!(
        original_or_fork.contains(&sessions[1].to_owned()),
        "{sessions:?}"
    );
}
