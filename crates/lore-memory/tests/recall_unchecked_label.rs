//! What a recalled note is labelled when there is no working tree to check it
//! against. A note that has anchors has code to check; a note with no anchors
//! does not. The two must not share a label.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::fs;

use lore_memory::recall::recall;
use lore_memory::repo::RepoContext;
use lore_memory::vault::Vault;

const ANCHORED: &str = "---\ntype: fact\ntitle: Migrations keep a log\n\
anchors:\n  - path: src/db.rs\n    lines: 2-3\n    hash: b3:0123456789abcdef\n\
---\nEvery migration writes a log line.\n";

const UNANCHORED: &str =
    "---\ntype: fact\ntitle: Migrations are global\n---\nEvery migration is logged.\n";

#[test]
fn an_anchored_note_is_not_called_unanchored_without_a_working_tree() {
    let dir = tempfile::tempdir().unwrap();
    let (vault, _) = Vault::init(&dir.path().join("vault")).unwrap();
    vault.add(ANCHORED).unwrap();
    vault.add(UNANCHORED).unwrap();
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();

    let found = recall(&vault, "migration", &elsewhere, 10).unwrap();
    assert_eq!(found.context, RepoContext::NoRepo);
    let by_title = |title: &str| {
        found
            .notes
            .iter()
            .find(|r| r.note.title == title)
            .unwrap_or_else(|| panic!("{title} was not recalled"))
    };
    let anchored = by_title("Migrations keep a log");
    let unanchored = by_title("Migrations are global");
    assert_eq!(anchored.note.anchors.len(), 1, "the note does have anchors");
    assert!(
        anchored.anchors.is_empty(),
        "there is no working tree, so nothing was checked"
    );
    assert_ne!(
        anchored.freshness, unanchored.freshness,
        "a note whose code is unchecked is labelled the same as a note with no code to check"
    );
}
