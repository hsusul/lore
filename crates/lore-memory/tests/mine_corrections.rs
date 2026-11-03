//! The corrections miner reads the archive and finds the user turns that
//! correct the agent: deterministic signals only, every text redacted.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, Signal};

/// What a test checks of a candidate.
type Seen = (
    String,
    String,
    Option<String>,
    Option<i64>,
    Signal,
    Option<String>,
);

#[test]
fn a_user_turn_that_corrects_the_previous_assistant_turn_is_a_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let conn = archive::codex_archive(
        dir.path(),
        &[
            &[
                ("user", "add a retry to the client"),
                ("assistant", "I'll install axios with npm."),
                ("user", "no, use pnpm not npm in this repo"),
                ("assistant", "Switched to pnpm."),
                ("user", "now add a timeout"),
            ],
            &[
                ("user", "don't touch the generated files"),
                ("assistant", "Understood."),
                (
                    "user",
                    "Stop editing src/gen.rs, I told you it is generated",
                ),
            ],
        ],
    );
    let found = corrections(&conn).unwrap();
    let got: Vec<Seen> = found
        .into_iter()
        .map(|c| (c.session, c.text, c.preceding, c.at_ms, c.signal, c.repo))
        .collect();
    let first = archive::session_id(&conn, 1);
    let second = archive::session_id(&conn, 2);
    assert_eq!(
        got,
        [
            (
                first,
                "no, use pnpm not npm in this repo".to_owned(),
                Some("I'll install axios with npm.".to_owned()),
                Some(1_785_578_403_000),
                Signal::Correction,
                None,
            ),
            (
                second,
                "Stop editing src/gen.rs, I told you it is generated".to_owned(),
                Some("Understood.".to_owned()),
                Some(1_785_664_803_000),
                Signal::Correction,
                None,
            ),
        ]
    );
}

#[test]
fn a_user_turn_that_repeats_an_earlier_instruction_is_a_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let conn = archive::codex_archive(
        dir.path(),
        &[&[
            ("user", "run the full test suite before you commit"),
            ("assistant", "Committed the change."),
            ("user", "please run the full test suite before you commit"),
        ]],
    );
    let found = corrections(&conn).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].signal, Signal::Repeat);
    assert_eq!(
        found[0].text,
        "please run the full test suite before you commit"
    );
    assert_eq!(found[0].agent, "codex");
}

#[test]
fn ordinary_turns_harness_text_and_long_prompts_are_not_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let long = format!("no, {}", "rewrite the whole module ".repeat(30));
    let conn = archive::codex_archive(
        dir.path(),
        &[&[
            ("user", "no worries, start with the parser"),
            ("assistant", "Starting."),
            (
                "user",
                "<environment_context>no, do not</environment_context>",
            ),
            ("assistant", "Ok."),
            ("user", &long),
            ("assistant", "Ok."),
            ("user", "looks good, ship it"),
        ]],
    );
    // The first turn says "no" but follows no assistant turn.
    assert_eq!(corrections(&conn).unwrap(), Vec::new());
}

#[test]
fn a_secret_in_a_correction_is_masked() {
    let dir = tempfile::tempdir().unwrap();
    let key = concat!("AKIA", "Z7Q4R2T6Y8U1W3E5");
    let text = format!("no, use the staging key {key} instead");
    let conn = archive::codex_archive(
        dir.path(),
        &[&[
            ("user", "deploy it"),
            ("assistant", "Deployed to prod."),
            ("user", &text),
        ]],
    );
    let found = corrections(&conn).unwrap();
    assert_eq!(found.len(), 1);
    assert!(!found[0].text.contains(key), "{}", found[0].text);
    assert!(
        found[0].text.starts_with("no, use the staging key "),
        "{}",
        found[0].text
    );
}
