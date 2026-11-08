//! Corrections the miner used to lose: one typed around a pasted block, which
//! the harness stripping dropped with the paste, and one flagged as a
//! correction whose lesson is dropped for stating no rule, which then reached
//! neither a lesson nor the replies a lessons run reads.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, lessons, read_turns, replies_in, Candidate, Limits, Signal};

const S: &str = "44444444-4444-4444-8444-444444444444";

fn session(dir: &std::path::Path, turns: &[(&str, &str)]) -> rusqlite::Connection {
    let stamps: Vec<String> = (0..turns.len())
        .map(|n| format!("2026-08-05T10:00:{n:02}.000Z"))
        .collect();
    let turns: Vec<archive::Turn<'_>> = turns
        .iter()
        .zip(&stamps)
        .map(|((role, text), ts)| (*role, *text, ts.as_str()))
        .collect();
    archive::claude_archive(dir, &[(S, &turns)])
}

/// What a lessons run reads: the receipts of the lessons the miner keeps,
/// and the ordinary replies.
fn run_reads(conn: &rusqlite::Connection) -> Vec<Candidate> {
    let turns = read_turns(conn).unwrap();
    let mut read: Vec<Candidate> = lessons(corrections(conn).unwrap())
        .into_iter()
        .flat_map(|l| l.receipts)
        .collect();
    read.extend(replies_in(&turns, &Limits::default(), i64::MIN));
    read
}

const PASTE: &str = "<pasted_content id=\"p1\">\nerror: linker cc not found\nnote: see the build log\n</pasted_content id=\"p1\">";

#[test]
fn words_typed_after_a_paste_are_mined_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let turn = format!("{PASTE}\n\nno, never install a global toolchain to fix this");
    let conn = session(
        dir.path(),
        &[
            ("user", "build the parser crate"),
            ("assistant", "Installing a global toolchain now."),
            ("user", &turn),
            ("assistant", "Undone."),
        ],
    );
    let found = corrections(&conn).unwrap();
    let mined: Vec<(&str, Signal)> = found.iter().map(|c| (c.text.as_str(), c.signal)).collect();
    assert_eq!(
        mined,
        [(
            "no, never install a global toolchain to fix this",
            Signal::Correction
        )]
    );
    assert_eq!(
        found[0].preceding.as_deref(),
        Some("Installing a global toolchain now.")
    );
}

#[test]
fn words_typed_before_a_paste_are_mined_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let turn = format!("that's not the error I meant, it's this one:\n{PASTE}");
    let conn = session(
        dir.path(),
        &[
            ("user", "build the parser crate"),
            ("assistant", "Fixed the missing import."),
            ("user", &turn),
        ],
    );
    let texts: Vec<String> = corrections(&conn)
        .unwrap()
        .into_iter()
        .map(|c| c.text)
        .collect();
    assert_eq!(texts, ["that's not the error I meant, it's this one:"]);
}

#[test]
fn a_paste_with_nothing_typed_stays_excluded() {
    let dir = tempfile::tempdir().unwrap();
    let conn = session(
        dir.path(),
        &[
            ("user", "build the parser crate"),
            ("assistant", "Built it."),
            ("user", PASTE),
            ("assistant", "That log shows a missing linker."),
        ],
    );
    assert!(corrections(&conn).unwrap().is_empty());
    let turns = read_turns(&conn).unwrap();
    assert!(replies_in(&turns, &Limits::default(), i64::MIN).is_empty());
}

#[test]
fn a_correction_whose_lesson_is_dropped_is_read_once_as_a_reply() {
    let dir = tempfile::tempdir().unwrap();
    let conn = session(
        dir.path(),
        &[
            ("user", "draft the release notes"),
            ("assistant", "Here are the release notes in a table."),
            ("user", "no, that's wrong, the second draft was better"),
            ("assistant", "Restored the second draft."),
        ],
    );
    let flagged = corrections(&conn).unwrap();
    assert_eq!(flagged.len(), 1);
    assert_eq!(flagged[0].signal, Signal::Correction);
    // It states no rule, so no lesson keeps it ...
    assert!(lessons(flagged).is_empty());
    // ... and a lessons run reads it once, as a reply, with its context.
    let read = run_reads(&conn);
    assert_eq!(read.len(), 1);
    assert_eq!(
        read[0].text,
        "no, that's wrong, the second draft was better"
    );
    assert_eq!(read[0].signal, Signal::Reply);
    assert_eq!(
        read[0].preceding.as_deref(),
        Some("Here are the release notes in a table.")
    );
}

#[test]
fn a_correction_a_lesson_keeps_is_not_also_a_reply() {
    let dir = tempfile::tempdir().unwrap();
    let conn = session(
        dir.path(),
        &[
            ("user", "add the dependency"),
            ("assistant", "Ran npm install."),
            ("user", "no, use pnpm not npm"),
            ("assistant", "Switched to pnpm."),
        ],
    );
    let read = run_reads(&conn);
    let texts: Vec<(&str, Signal)> = read.iter().map(|c| (c.text.as_str(), c.signal)).collect();
    assert_eq!(texts, [("no, use pnpm not npm", Signal::Correction)]);
}
