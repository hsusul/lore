//! Recall by file: the notes anchored to the one file an agent is about to
//! change, wherever that file was renamed from.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::fs;
use std::path::Path;

use common::{commit, git, repo};
use lore_memory::anchor::WorkTree;
use lore_memory::note::RelPath;
use lore_memory::recall::anchored_to;
use lore_memory::repo::RepoContext;
use lore_memory::vault::Vault;

const REMOTE: &str = "https://github.com/example/app.git";

fn note(title: &str, scope: &str, status: &str, anchors: &[(&str, &str)]) -> String {
    let mut text =
        format!("---\ntype: pitfall\ntitle: {title}\nscope: {scope}\nstatus: {status}\n");
    if !anchors.is_empty() {
        text.push_str("anchors:\n");
        for (path, lines) in anchors {
            text.push_str(&format!("  - path: {path}\n    lines: {lines}\n"));
        }
    }
    text.push_str("---\nBody.\n");
    text
}

/// A repo with `src/lib.rs` and `src/db.rs`, and a vault with notes anchored
/// to each, one retired, and one global note with no anchors.
fn setup(dir: &Path) -> (std::path::PathBuf, Vault) {
    let app = dir.join("app");
    repo(&app, Some(REMOTE));
    commit(&app, "src/lib.rs", "a\nb\nfn migrate() {}\n", "lib");
    commit(&app, "src/db.rs", "fn connect() {}\n", "db");
    let key = RepoContext::discover(&app).key().cloned().unwrap();
    let scope = format!("repo:{}", key.as_str());

    let (vault, _) = Vault::init(&dir.join("vault")).unwrap();
    let tree = WorkTree::new(&app);
    for text in [
        note(
            "Migrate runs once",
            &scope,
            "active",
            &[("src/lib.rs", "3")],
        ),
        note(
            "Connect before migrate",
            &scope,
            "active",
            &[("src/db.rs", "1"), ("src/lib.rs", "1-2")],
        ),
        note("Only the database", &scope, "active", &[("src/db.rs", "1")]),
        note(
            "Retired lib note",
            &scope,
            "retired",
            &[("src/lib.rs", "3")],
        ),
        note("Global and unanchored", "global", "active", &[]),
    ] {
        vault
            .add_with(&text, |note| {
                for anchor in &mut note.anchors {
                    *anchor = tree.pin(anchor)?;
                }
                Ok(())
            })
            .unwrap();
    }
    (app, vault)
}

fn titles(vault: &Vault, cwd: &Path, path: &str) -> Vec<(String, &'static str)> {
    let path = RelPath::parse(path).unwrap();
    anchored_to(vault, cwd, &path, 10)
        .unwrap()
        .into_iter()
        .map(|r| (r.note.title, r.freshness.label()))
        .collect()
}

fn owned(expected: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    expected.iter().map(|(t, l)| (t.to_string(), *l)).collect()
}

#[test]
fn only_active_notes_anchored_to_the_file_are_returned() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    assert_eq!(
        titles(&vault, &app, "src/lib.rs"),
        owned(&[
            ("Connect before migrate", "fresh"),
            ("Migrate runs once", "fresh"),
        ])
    );
    assert_eq!(
        titles(&vault, &app.join("src"), "src/db.rs"),
        owned(&[
            ("Connect before migrate", "fresh"),
            ("Only the database", "fresh"),
        ])
    );
    assert_eq!(titles(&vault, &app, "src/none.rs"), owned(&[]));
}

#[test]
fn a_note_whose_lines_changed_is_labelled_and_listed_last() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    fs::write(app.join("src/lib.rs"), "a\nb\nfn migrate_twice() {}\n").unwrap();
    assert_eq!(
        titles(&vault, &app, "src/lib.rs"),
        owned(&[
            ("Connect before migrate", "fresh"),
            ("Migrate runs once", "changed"),
        ])
    );
}

#[test]
fn a_note_follows_its_file_to_a_new_name() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    git(&app, &["mv", "src/lib.rs", "src/core.rs"]);
    git(&app, &["commit", "-q", "-m", "rename"]);
    assert_eq!(
        titles(&vault, &app, "src/core.rs"),
        owned(&[
            ("Connect before migrate", "moved"),
            ("Migrate runs once", "moved"),
        ])
    );
    assert_eq!(titles(&vault, &app, "src/lib.rs"), owned(&[]));
}

#[test]
fn outside_the_repo_nothing_is_anchored() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, vault) = setup(dir.path());
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    assert_eq!(titles(&vault, &elsewhere, "src/lib.rs"), owned(&[]));
}

#[test]
fn the_limit_keeps_the_first_notes() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    let path = RelPath::parse("src/lib.rs").unwrap();
    let found = anchored_to(&vault, &app, &path, 1).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].note.title, "Connect before migrate");
}
