//! Every active note in the vault, for browsing: each checked against its own
//! repository's working tree when the caller names it, else left unchecked.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::fs;
use std::path::Path;

use common::{commit, repo};
use lore_memory::anchor::WorkTree;
use lore_memory::recall::all_active;
use lore_memory::repo::RepoContext;
use lore_memory::vault::Vault;

const REMOTE: &str = "https://github.com/example/app.git";
const ALPHA: &str = "01J0000000000000000000000A";
const BETA: &str = "01J0000000000000000000000B";
const GLOBAL: &str = "01J0000000000000000000000C";
const OTHER: &str = "01J0000000000000000000000D";
const RETIRED: &str = "01J0000000000000000000000E";

fn note(id: &str, title: &str, scope: &str, status: &str, anchor: Option<&str>) -> String {
    let mut text =
        format!("---\nid: {id}\ntype: fact\ntitle: {title}\nscope: {scope}\nstatus: {status}\n");
    if let Some(path) = anchor {
        text.push_str(&format!("anchors:\n  - path: {path}\n    lines: 1\n"));
    }
    text.push_str("---\nNothing in common with the other notes.\n");
    text
}

/// A repo and a vault with two repo notes (one whose anchor then changes),
/// one global note, one note for another repo, and one retired repo note.
fn setup(dir: &Path) -> (std::path::PathBuf, Vault) {
    let app = dir.join("app");
    repo(&app, Some(REMOTE));
    commit(&app, "src/db.rs", "fn connect() {}\n", "db");
    commit(&app, "src/lib.rs", "fn migrate() {}\n", "lib");
    let key = RepoContext::discover(&app).key().cloned().unwrap();
    let scope = format!("repo:{}", key.as_str());
    let (vault, _) = Vault::init(&dir.join("vault")).unwrap();
    let tree = WorkTree::new(&app);
    let notes = [
        note(
            ALPHA,
            "Alpha opens a connection",
            &scope,
            "active",
            Some("src/db.rs"),
        ),
        note(
            BETA,
            "Beta runs the migration",
            &scope,
            "active",
            Some("src/lib.rs"),
        ),
        note(GLOBAL, "Gamma holds everywhere", "global", "active", None),
        note(
            OTHER,
            "Delta is another repo",
            "repo:aaaaaaaaaaaaaaaa",
            "active",
            None,
        ),
        note(RETIRED, "Epsilon was retired", &scope, "retired", None),
    ];
    for text in notes {
        vault
            .add_with(&text, |note| {
                for anchor in &mut note.anchors {
                    *anchor = tree.pin(anchor)?;
                }
                Ok(())
            })
            .unwrap();
    }
    fs::write(app.join("src/db.rs"), "fn connect_twice() {}\n").unwrap();
    (app, vault)
}

fn labels(found: &[lore_memory::recall::Recalled]) -> Vec<(String, &'static str)> {
    found
        .iter()
        .map(|r| (r.note.title.clone(), r.freshness.label()))
        .collect()
}

#[test]
fn every_active_note_is_listed_and_checked_in_its_own_repository() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    let mut found = labels(&all_active(&vault, &[app]).unwrap());
    found.sort();
    assert_eq!(
        found,
        [
            ("Alpha opens a connection".to_owned(), "changed"),
            ("Beta runs the migration".to_owned(), "fresh"),
            ("Delta is another repo".to_owned(), "unanchored"),
            ("Gamma holds everywhere".to_owned(), "unanchored"),
        ]
    );
}

#[test]
fn a_note_whose_repository_was_not_named_is_unchecked() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, vault) = setup(dir.path());
    let mut found = labels(&all_active(&vault, &[]).unwrap());
    found.sort();
    assert_eq!(
        found,
        [
            ("Alpha opens a connection".to_owned(), "unchecked"),
            ("Beta runs the migration".to_owned(), "unchecked"),
            ("Delta is another repo".to_owned(), "unanchored"),
            ("Gamma holds everywhere".to_owned(), "unanchored"),
        ]
    );
}
