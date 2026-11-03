//! "make sure", "please always", "from now on", and "going forward" state a
//! standing rule, typos and all, even when no agent turn comes before them.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, states_rule, Signal};

const RULES: &[&str] = &[
    "make sure that docs are never tracked",
    "make sure that none of the docs are pushed",
    "make sure teh docs arnt in either",
    "please always run the tests first",
    "from now on keep the changelog short",
    "going forward squash before merging",
    "ok and make sure the readme stays short",
];

#[test]
fn rule_phrases_are_candidates_and_state_a_rule() {
    let dir = tempfile::tempdir().unwrap();
    // The first rule opens the session: no agent turn comes before it.
    let mut session = vec![("user", RULES[0])];
    for rule in &RULES[1..] {
        session.push(("assistant", "Done."));
        session.push(("user", rule));
    }
    let conn = archive::codex_archive(dir.path(), &[&session]);
    let found: Vec<(String, Signal)> = corrections(&conn)
        .unwrap()
        .into_iter()
        .map(|c| (c.text, c.signal))
        .collect();
    let expected: Vec<(String, Signal)> = RULES
        .iter()
        .map(|r| ((*r).to_owned(), Signal::Rule))
        .collect();
    assert_eq!(found, expected);
    for rule in RULES {
        assert!(states_rule(rule), "{rule}");
    }
}

#[test]
fn a_correction_that_also_says_make_sure_is_still_a_correction() {
    let dir = tempfile::tempdir().unwrap();
    let conn = archive::codex_archive(
        dir.path(),
        &[&[
            ("user", "ship it"),
            ("assistant", "Opened a PR."),
            ("user", "dont make a pr just push to main, make sure of it"),
        ]],
    );
    let found = corrections(&conn).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].signal, Signal::Correction);
}
