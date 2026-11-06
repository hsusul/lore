//! The considered ledger: which mined corrections a lessons run already read,
//! keyed by a correction id that does not depend on how lessons group.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use std::collections::BTreeSet;

use lore_memory::considered::{
    self, considered, mark, record, unconsidered, CorrectionId, CONSIDERED_FILE,
};
use lore_memory::inbox;
use lore_memory::mine::{corrections, lessons, read_turns};
use lore_memory::vault::Vault;

fn pnpm_archive(dir: &std::path::Path) -> rusqlite::Connection {
    archive::codex_archive(
        dir,
        &[
            &[
                ("user", "add axios"),
                ("assistant", "Running npm install axios."),
                ("user", "no, use pnpm not npm"),
            ],
            &[
                ("user", "add lodash"),
                ("assistant", "Running npm install lodash."),
                ("user", "wrong tool, use pnpm instead of npm"),
            ],
            &[
                ("user", "add dayjs"),
                ("assistant", "npm install dayjs done."),
                ("user", "I told you, pnpm not npm"),
            ],
        ],
    )
}

#[test]
fn a_correction_keeps_its_id_however_lessons_group_it() {
    let dir = tempfile::tempdir().unwrap();
    let conn = pnpm_archive(dir.path());
    let found = corrections(&conn).unwrap();
    let newest = found
        .iter()
        .find(|c| c.text == "I told you, pnpm not npm")
        .unwrap();
    let id = CorrectionId::of(newest);
    assert!(
        id.as_str().starts_with("cr-") && id.as_str().len() == 19,
        "{id}"
    );
    assert_eq!(CorrectionId::parse(id.as_str()), Some(id.clone()));
    assert_eq!(CorrectionId::parse("cr-XYZ"), None);

    // All three in one lesson, then without the oldest: the lesson id moves
    // with its oldest receipt, the correction id does not.
    let whole = lessons(found.clone());
    let without_oldest = lessons(found[1..].to_vec());
    assert_ne!(whole[0].id, without_oldest[0].id);
    for grouping in [&whole, &without_oldest] {
        let marked = mark(grouping.clone(), |_| true, true);
        let ids: Vec<&CorrectionId> = marked[0].receipts().map(|(_, id, _)| id).collect();
        assert_eq!(ids[0], &id);
    }
}

#[test]
fn the_ledger_is_a_set_that_survives_a_torn_line() {
    let dir = tempfile::tempdir().unwrap();
    let (vault, _) = Vault::init(dir.path()).unwrap();
    assert!(considered(&vault).unwrap().is_empty());
    let a = CorrectionId::parse("cr-0123456789abcdef").unwrap();
    let b = CorrectionId::parse("cr-fedcba9876543210").unwrap();
    record(&vault, [&a, &b], "run-1").unwrap();
    record(&vault, [&a], "run-2").unwrap();
    assert_eq!(
        considered(&vault).unwrap(),
        BTreeSet::from([a.clone(), b.clone()])
    );

    let path = dir.path().join("_inbox").join(CONSIDERED_FILE);
    let text = std::fs::read_to_string(&path).unwrap();
    let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(first["id"], a.as_str());
    assert_eq!(first["run"], "run-1");
    assert!(first["considered_at"].as_str().unwrap().contains('T'));

    // A crash tore the last line: it is ignored, and the next id still lands.
    let mut torn = std::fs::read(&path).unwrap();
    torn.extend_from_slice(br#"{"id":"cr-1111111111111111","consid"#);
    std::fs::write(&path, torn).unwrap();
    assert_eq!(considered(&vault).unwrap().len(), 2);
    let c = CorrectionId::parse("cr-2222222222222222").unwrap();
    record(&vault, [&c], "run-3").unwrap();
    assert_eq!(considered(&vault).unwrap(), BTreeSet::from([a, b, c]));

    // The ledger is not a proposal file.
    let (proposals, skipped) = inbox::proposals(&vault).unwrap();
    assert!(proposals.is_empty() && skipped.is_empty(), "{skipped:?}");
}

#[test]
fn only_lessons_with_an_unconsidered_receipt_are_new() {
    let dir = tempfile::tempdir().unwrap();
    let conn = pnpm_archive(dir.path());
    let turns = read_turns(&conn).unwrap();
    let pending = unconsidered(&turns, &BTreeSet::new());
    assert_eq!(pending.len(), 3);

    let found = lessons(corrections(&conn).unwrap());
    let ids: Vec<CorrectionId> = found[0].receipts.iter().map(CorrectionId::of).collect();
    // The two older receipts were read; the newest was not.
    let seen: BTreeSet<CorrectionId> = ids[1..].iter().cloned().collect();
    let marked = mark(found.clone(), |id| !seen.contains(id), false);
    let flags: Vec<bool> = marked[0].receipts().map(|(_, _, new)| new).collect();
    assert_eq!(flags, [true, false, false]);
    assert_eq!(
        unconsidered(&turns, &seen),
        BTreeSet::from([ids[0].clone()])
    );

    let all: BTreeSet<CorrectionId> = ids.iter().cloned().collect();
    assert!(mark(found.clone(), |id| !all.contains(id), false).is_empty());
    assert_eq!(mark(found, |id| !all.contains(id), true).len(), 1);
    assert!(unconsidered(&turns, &all).is_empty());
}

#[test]
fn an_id_file_lists_one_id_per_line() {
    let ids = considered::parse_ids(b"cr-0123456789abcdef\n\nnot an id\ncr-0123456789abcdef\r\n{\"id\":\"cr-aaaaaaaaaaaaaaaa\"}\ncr-bbbb");
    assert_eq!(
        ids.iter().map(CorrectionId::as_str).collect::<Vec<_>>(),
        ["cr-0123456789abcdef", "cr-aaaaaaaaaaaaaaaa"]
    );
}
