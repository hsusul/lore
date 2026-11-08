//! The person often sends two messages in a row while the agent works. Each
//! turn in such a run answers the agent turn before the run, so a correction
//! that comes second is mined with that agent turn as its context, and every
//! turn in the run is a candidate at most once.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use std::collections::BTreeSet;

use lore_memory::mine::{corrections, lessons, read_turns, replies_in, Limits, Signal};

const S: &str = "33333333-3333-4333-8333-333333333333";

fn at(s: u32) -> String {
    format!("2026-08-05T10:00:{s:02}.000Z")
}

fn session(dir: &std::path::Path, turns: &[(&str, &str)]) -> rusqlite::Connection {
    let stamps: Vec<String> = (0..turns.len())
        .map(|n| at(u32::try_from(n).unwrap()))
        .collect();
    let turns: Vec<archive::Turn<'_>> = turns
        .iter()
        .zip(&stamps)
        .map(|((role, text), ts)| (*role, *text, ts.as_str()))
        .collect();
    archive::claude_archive(dir, &[(S, &turns)])
}

#[test]
fn a_correction_second_in_a_run_is_mined_with_the_agent_turn_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let conn = session(
        dir.path(),
        &[
            ("user", "tidy up the billing helpers"),
            ("assistant", "Rewriting the ledger exporter now."),
            ("user", "also the invoice template"),
            ("user", "don't touch the ledger exporter, revert it"),
            ("assistant", "Reverted the ledger exporter."),
        ],
    );
    let found = corrections(&conn).unwrap();
    let mined: Vec<(&str, Signal)> = found.iter().map(|c| (c.text.as_str(), c.signal)).collect();
    assert_eq!(
        mined,
        [(
            "don't touch the ledger exporter, revert it",
            Signal::Correction
        )]
    );
    assert_eq!(
        found[0].preceding.as_deref(),
        Some("Rewriting the ledger exporter now.")
    );
    assert_eq!(
        found[0].following.as_deref(),
        Some("Reverted the ledger exporter.")
    );

    // The other turn in the run is a reply to the same agent turn; the
    // correction is not offered again as a reply.
    let turns = read_turns(&conn).unwrap();
    let replies = replies_in(&turns, &Limits::default(), i64::MIN);
    let texts: Vec<&str> = replies.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["also the invoice template"]);
    assert_eq!(
        replies[0].preceding.as_deref(),
        Some("Rewriting the ledger exporter now.")
    );
}

#[test]
fn a_single_turn_after_the_agent_behaves_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let conn = session(
        dir.path(),
        &[
            ("user", "tidy up the billing helpers"),
            ("assistant", "Rewriting the ledger exporter now."),
            ("user", "no, leave the ledger exporter alone"),
            ("assistant", "Left it alone."),
        ],
    );
    let found = corrections(&conn).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].signal, Signal::Correction);
    assert_eq!(
        found[0].preceding.as_deref(),
        Some("Rewriting the ledger exporter now.")
    );
    // It states no rule, so no lesson keeps it: a lessons run reads it as a
    // reply instead.
    let turns = read_turns(&conn).unwrap();
    let replies = replies_in(&turns, &Limits::default(), i64::MIN);
    let replied: Vec<(&str, Signal)> = replies
        .iter()
        .map(|c| (c.text.as_str(), c.signal))
        .collect();
    assert_eq!(
        replied,
        [("no, leave the ledger exporter alone", Signal::Reply)]
    );
}

#[test]
fn a_run_yields_each_turn_once_and_harness_text_ends_it() {
    let dir = tempfile::tempdir().unwrap();
    let conn = session(
        dir.path(),
        &[
            ("user", "no, use the staging bucket"),
            ("user", "and keep the old keys"),
            ("assistant", "Uploading to the staging bucket."),
            ("user", "no, the other staging bucket"),
            ("user", "wrong region too"),
            ("user", "and rename the folder"),
            ("user", "[Request interrupted by user]"),
            ("user", "no, stop renaming things"),
            ("assistant", "Stopped."),
        ],
    );
    let turns = read_turns(&conn).unwrap();
    let found = corrections(&conn).unwrap();
    let replies = replies_in(&turns, &Limits::default(), i64::MIN);

    // Before any agent turn there is nothing to correct or reply to. After
    // the interrupt notice the run is over, so the last turn answers none.
    let corrected: Vec<&str> = found
        .iter()
        .filter(|c| c.signal == Signal::Correction)
        .map(|c| c.text.as_str())
        .collect();
    assert_eq!(
        corrected,
        ["no, the other staging bucket", "wrong region too"]
    );
    // Neither correction states a rule, so no lesson keeps them and they are
    // offered as replies, with the run's other reply.
    let replied: Vec<&str> = replies.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(
        replied,
        [
            "no, the other staging bucket",
            "wrong region too",
            "and rename the folder"
        ]
    );

    // A lessons run reads each turn once: a lesson's receipt or a reply.
    let receipts: Vec<_> = lessons(found)
        .into_iter()
        .flat_map(|l| l.receipts)
        .collect();
    let mut seen = BTreeSet::new();
    for c in receipts.iter().chain(&replies) {
        assert!(seen.insert(c.text.clone()), "counted twice: {}", c.text);
    }
}
