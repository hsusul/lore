//! A rule said once still matters. Lessons backed by one conversation are kept
//! when they state a rule, ranked below the ones said in several, and each
//! lesson says how many distinct conversations back it.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, lessons};

fn after(action: &'static str, said: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![("user", "go ahead"), ("assistant", action), ("user", said)]
}

#[test]
fn single_conversation_rules_rank_below_repeated_ones() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = [
        after("Running npm install.", "no, use pnpm not npm"),
        after("Shall I run it?", "Stop asking me for permission"),
        after("Opened a PR.", "dont make a pr just push to main"),
        after("Running npm install.", "I told you, pnpm not npm"),
        after("Opened a PR.", "dont pr just puhs"),
        after("Here is a long answer.", "no, the other one"),
    ];
    let refs: Vec<&[(&str, &str)]> = sessions.iter().map(Vec::as_slice).collect();
    let conn = archive::codex_archive(dir.path(), &refs);
    let found: Vec<(String, usize)> = lessons(corrections(&conn).unwrap())
        .into_iter()
        .map(|l| (l.text, l.conversations))
        .collect();
    assert_eq!(
        found,
        [
            ("I told you, pnpm not npm".to_owned(), 2),
            ("dont pr just puhs".to_owned(), 1),
            ("dont make a pr just push to main".to_owned(), 1),
            ("Stop asking me for permission".to_owned(), 1),
        ]
    );
}
