//! Ordinary replies: the person's turns right after the agent's that no rule
//! flagged, offered to a lessons run for its agent to judge.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use std::collections::BTreeSet;

use lore_memory::considered::{unconsidered, CorrectionId};
use lore_memory::mine::{read_turns, replies_in, reply_lessons, Limits, Signal};

/// 2026-08-02T00:00:00Z: after the first session, before the second.
const AUG_2: i64 = 1_785_628_800_000;

fn sessions(dir: &std::path::Path) -> rusqlite::Connection {
    let at = |day: u32, s: u32| format!("2026-08-{day:02}T10:00:{s:02}.000Z");
    let old: Vec<(String, String, String)> = vec![
        ("user".into(), "fix the parser".into(), at(1, 0)),
        (
            "assistant".into(),
            "Fixed it with a regex.".into(),
            at(1, 1),
        ),
        (
            "user".into(),
            "looks fine but rename the helper".into(),
            at(1, 2),
        ),
    ];
    let new: Vec<(String, String, String)> = [
        ("user", "start"),
        ("assistant", "Done with part A."),
        ("user", "nice, now the docs"),
        ("user", "and the changelog too"),
        ("assistant", "Done with part B."),
        ("user", "no, use pnpm not npm"),
        ("assistant", "Switched to pnpm."),
        ("user", "/review the diff"),
        ("assistant", "Reviewed it."),
        (
            "user",
            "This session is being continued from a previous conversation that ran out of context.",
        ),
        ("assistant", "Resuming the work."),
        ("user", "ok ship it"),
        ("assistant", "Shipped."),
    ]
    .iter()
    .enumerate()
    .map(|(n, (role, text))| {
        (
            (*role).to_owned(),
            (*text).to_owned(),
            at(3, u32::try_from(n).unwrap()),
        )
    })
    .collect();
    fn borrow(turns: &[(String, String, String)]) -> Vec<archive::Turn<'_>> {
        turns
            .iter()
            .map(|(r, t, ts)| (r.as_str(), t.as_str(), ts.as_str()))
            .collect()
    }
    let (old, new) = (borrow(&old), borrow(&new));
    archive::claude_archive(
        dir,
        &[
            ("11111111-1111-4111-8111-111111111111", &old),
            ("22222222-2222-4222-8222-222222222222", &new),
        ],
    )
}

fn texts(found: &[lore_memory::mine::Candidate]) -> Vec<&str> {
    found.iter().map(|c| c.text.as_str()).collect()
}

#[test]
fn picks_plain_replies_after_an_agent_turn_inside_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let conn = sessions(dir.path());
    let turns = read_turns(&conn).unwrap();
    let found = replies_in(&turns, &Limits::default(), AUG_2);
    // Not the correction the rules picked, the slash command, the
    // continuation summary, the user turn after a user turn, or the reply
    // older than the window.
    assert_eq!(texts(&found), ["nice, now the docs", "ok ship it"]);
    assert!(found.iter().all(|c| c.signal == Signal::Reply));
    assert_eq!(found[0].preceding.as_deref(), Some("Done with part A."));
    assert_eq!(found[1].following.as_deref(), Some("Shipped."));

    let everything = replies_in(&turns, &Limits::default(), i64::MIN);
    assert_eq!(
        texts(&everything),
        [
            "looks fine but rename the helper",
            "nice, now the docs",
            "ok ship it"
        ]
    );
}

#[test]
fn a_reply_keeps_its_id_and_stands_alone() {
    let dir = tempfile::tempdir().unwrap();
    let conn = sessions(dir.path());
    let turns = read_turns(&conn).unwrap();
    let ids = |since| -> Vec<CorrectionId> {
        replies_in(&turns, &Limits::default(), since)
            .iter()
            .map(CorrectionId::of)
            .collect()
    };
    let first = ids(AUG_2);
    assert_eq!(first, ids(AUG_2));
    assert_eq!(&ids(i64::MIN)[1..], &first[..]);

    // Each reply is its own lesson, newest first.
    let lessons = reply_lessons(replies_in(&turns, &Limits::default(), AUG_2));
    let shape: Vec<(&str, usize)> = lessons
        .iter()
        .map(|l| (l.text.as_str(), l.receipts.len()))
        .collect();
    assert_eq!(shape, [("ok ship it", 1), ("nice, now the docs", 1)]);

    // A run reads the rule-mined corrections and the window's replies.
    let pending = unconsidered(&turns, &BTreeSet::new(), AUG_2);
    assert_eq!(pending.len(), 3, "{pending:?}");
    assert!(first.iter().all(|id| pending.contains(id)));
    let seen: BTreeSet<CorrectionId> = pending.iter().cloned().collect();
    assert!(unconsidered(&turns, &seen, AUG_2).is_empty());
    assert_eq!(unconsidered(&turns, &seen, i64::MIN).len(), 1);
}
