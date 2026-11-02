//! Recall with no query: every active note that applies in the directory,
//! ranked by freshness like any recall, at most `limit`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::fs;
use std::path::Path;

use common::{commit, repo};
use lore_memory::anchor::WorkTree;
use lore_memory::recall::recall;
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

fn ids(vault: &Vault, query: &str, cwd: &Path, limit: usize) -> Vec<(String, &'static str)> {
    recall(vault, query, cwd, limit)
        .unwrap()
        .notes
        .into_iter()
        .map(|r| (r.note.id.as_str().to_string(), r.freshness.label()))
        .collect()
}

fn owned(expected: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    expected.iter().map(|(i, l)| (i.to_string(), *l)).collect()
}

#[test]
fn an_empty_query_recalls_every_active_note_in_scope() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    let expected = owned(&[(GLOBAL, "unanchored"), (BETA, "fresh"), (ALPHA, "changed")]);
    assert_eq!(ids(&vault, "", &app.join("src"), 10), expected);
    assert_eq!(ids(&vault, "  ?! ", &app, 10), expected);
}

#[test]
fn an_empty_query_outside_a_repo_recalls_global_notes_only() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, vault) = setup(dir.path());
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    assert_eq!(
        ids(&vault, "", &elsewhere, 10),
        owned(&[(GLOBAL, "unanchored")])
    );
}

#[test]
fn an_empty_query_honors_the_limit_in_path_order() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    assert_eq!(
        ids(&vault, "", &app, 2),
        owned(&[(GLOBAL, "unanchored"), (ALPHA, "changed")])
    );
    assert_eq!(ids(&vault, "", &app, 1), owned(&[(GLOBAL, "unanchored")]));
}

#[test]
fn an_empty_query_reports_a_note_it_cannot_parse() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    fs::write(vault.root().join("global/broken.md"), "---\ntype: fact\n").unwrap();
    let found = recall(&vault, "", &app, 10).unwrap();
    assert_eq!(found.notes.len(), 3);
    let skipped: Vec<(&str, String)> = found
        .skipped
        .iter()
        .map(|(path, error)| (path.as_str(), error.to_string()))
        .collect();
    assert_eq!(
        skipped,
        [(
            "global/broken.md",
            "not a note: the frontmatter has no closing `---` line".to_string()
        )]
    );
}
