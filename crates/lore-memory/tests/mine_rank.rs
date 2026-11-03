//! Candidates rank by how much they read like a standing rule, then by how
//! many conversations back them, then by recency. A one-off task order that
//! names a branch, a PR, or an issue sinks below a rule said once.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, lessons, rule_score};

const TASK: &str = "Push claude/exif-without-editing-input and open it as a PR stacked on \
#478. Don't ship the charging gate.";
const TASK_AGAIN: &str = "Push claude/exif-without-editing-input again and keep it stacked on \
#478. Don't ship the charging gate.";
const RULE: &str = "make sure that docs are never tracked";

#[test]
fn a_standing_rule_ranks_above_a_newer_repeated_task_order() {
    let dir = tempfile::tempdir().unwrap();
    let conn = archive::codex_archive(
        dir.path(),
        &[
            &[
                ("user", "go"),
                ("assistant", "Committed docs/ too."),
                ("user", RULE),
            ],
            &[
                ("user", "go"),
                ("assistant", "Opened a PR."),
                ("user", TASK),
            ],
            &[
                ("user", "go"),
                ("assistant", "Opened another PR."),
                ("user", TASK_AGAIN),
            ],
            &[
                ("user", "go"),
                ("assistant", "Shall I?"),
                ("user", "Stop asking me for permission"),
            ],
        ],
    );
    let ranked: Vec<(String, usize)> = lessons(corrections(&conn).unwrap())
        .into_iter()
        .map(|l| (l.text, l.conversations))
        .collect();
    assert_eq!(
        ranked,
        [
            (RULE.to_owned(), 1),
            ("Stop asking me for permission".to_owned(), 1),
            (TASK_AGAIN.to_owned(), 2),
        ]
    );
}

#[test]
fn redacted_tokens_urls_and_task_verbs_lower_the_score() {
    let scores: Vec<(&str, i32)> = [
        "make sure you merge the PR at «redacted:high-entropy» before lunch",
        "always check https://github.com/acme/app/pull/478 first",
        "make sure to commit and push every little step",
        "make sure that docs are never tracked",
        "make sure the readme stays short",
    ]
    .iter()
    .map(|t| (*t, rule_score(t)))
    .collect();
    assert_eq!(
        scores,
        [
            (
                "make sure you merge the PR at «redacted:high-entropy» before lunch",
                -2
            ),
            (
                "always check https://github.com/acme/app/pull/478 first",
                -1
            ),
            ("make sure to commit and push every little step", 2),
            ("make sure that docs are never tracked", 3),
            ("make sure the readme stays short", 3),
        ]
    );
}

#[test]
fn the_rule_score_is_fixed_and_explainable() {
    let scores: Vec<(&str, i32)> = [
        "from now on run the formatter first",
        "make sure that docs are never tracked",
        "always use pnpm",
        "Stop asking me for permission",
        "dont make a pr just push to main",
        "never edit src/generated/schema.rs",
        "fix issue 12 and close it",
        "rebase feature/login-form onto main",
        TASK,
    ]
    .iter()
    .map(|t| (*t, rule_score(t)))
    .collect();
    assert_eq!(
        scores,
        [
            ("from now on run the formatter first", 3),
            ("make sure that docs are never tracked", 3),
            ("always use pnpm", 3),
            ("Stop asking me for permission", 1),
            ("dont make a pr just push to main", 1),
            ("never edit src/generated/schema.rs", 2),
            ("fix issue 12 and close it", -5),
            ("rebase feature/login-form onto main", -5),
            (TASK, -8),
        ]
    );
}

#[test]
fn a_task_with_a_rule_tacked_on_scores_below_zero() {
    let scores: Vec<(&str, i32)> = [
        "delete the ios sim runtimes and make it so that it builds them on my ssd from now on",
        "install the deps and run the tests",
        "dont make a doc for it just give me the thing to copy from now on",
        "make sure to commit and push every little step",
    ]
    .iter()
    .map(|t| (*t, rule_score(t)))
    .collect();
    assert_eq!(
        scores,
        [
            (
                "delete the ios sim runtimes and make it so that it builds them on my ssd from now on",
                -1
            ),
            ("install the deps and run the tests", -3),
            (
                "dont make a doc for it just give me the thing to copy from now on",
                3
            ),
            ("make sure to commit and push every little step", 2),
        ]
    );
}
