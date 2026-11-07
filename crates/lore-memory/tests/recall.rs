//! Recall: scope from the directory, BM25 order, changed and gone notes last,
//! and a vault left exactly as it was.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::fs;
use std::path::Path;

use common::{commit, git, repo};
use lore_memory::anchor::{Freshness, WorkTree};
use lore_memory::recall::{recall, related};
use lore_memory::repo::RepoContext;
use lore_memory::vault::Vault;

const REMOTE: &str = "https://github.com/example/app.git";

fn note(title: &str, scope: &str, status: &str, anchors: &[(&str, &str)], body: &str) -> String {
    let mut text =
        format!("---\ntype: pitfall\ntitle: {title}\nscope: {scope}\nstatus: {status}\n");
    if !anchors.is_empty() {
        text.push_str("anchors:\n");
        for (path, lines) in anchors {
            text.push_str(&format!("  - path: {path}\n    lines: {lines}\n"));
        }
    }
    text.push_str(&format!("---\n{body}\n"));
    text
}

/// A repo with three files and a vault with six notes about migrations: one
/// per freshness, one global, one for another repo, and one retired.
fn setup(dir: &Path) -> (std::path::PathBuf, Vault) {
    let app = dir.join("app");
    repo(&app, Some(REMOTE));
    commit(&app, "src/lib.rs", "a\nb\nfn migrate() {}\n", "lib");
    commit(&app, "src/db.rs", "fn connect() {}\n", "db");
    commit(&app, "src/old.rs", "fn legacy() {}\n", "old");
    let key = RepoContext::discover(&app).key().cloned().unwrap();
    let scope = format!("repo:{}", key.as_str());

    let (vault, _) = Vault::init(&dir.join("vault")).unwrap();
    let tree = WorkTree::new(&app);
    let notes = [
        note(
            "Migration runs migrate once",
            &scope,
            "active",
            &[("src/lib.rs", "3")],
            "The migration entry point.",
        ),
        note(
            "Migration connects first",
            &scope,
            "active",
            &[("src/db.rs", "1")],
            "Open the migration connection.",
        ),
        note(
            "Migration legacy migration path",
            &scope,
            "active",
            &[("src/old.rs", "1")],
            "Old migration.",
        ),
        note(
            "Migration notes are global",
            "global",
            "active",
            &[],
            "Any migration, any repo.",
        ),
        note(
            "Migration in another repo",
            "repo:aaaaaaaaaaaaaaaa",
            "active",
            &[],
            "Not this repo's migration.",
        ),
        note(
            "Migration that was retired",
            &scope,
            "retired",
            &[],
            "A retired migration note.",
        ),
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
    fs::remove_file(app.join("src/old.rs")).unwrap();
    (app, vault)
}

fn labels(vault: &Vault, cwd: &Path) -> Vec<(String, &'static str)> {
    recall(vault, "migration", cwd, 10)
        .unwrap()
        .notes
        .into_iter()
        .map(|r| (r.note.title, r.freshness.label()))
        .collect()
}

fn owned(expected: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    expected.iter().map(|(t, l)| (t.to_string(), *l)).collect()
}

#[test]
fn recall_orders_fresh_then_changed_then_gone() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    let plain = vault.search("migration", 10).unwrap();
    assert_eq!(
        plain.first().map(|h| h.title.as_str()),
        Some("Migration legacy migration path"),
        "the gone note is the best BM25 match, so the sort is what moves it"
    );
    assert_eq!(
        labels(&vault, &app.join("src")),
        owned(&[
            ("Migration runs migrate once", "fresh"),
            ("Migration notes are global", "unanchored"),
            ("Migration connects first", "changed"),
            ("Migration legacy migration path", "gone"),
        ])
    );
}

#[test]
fn recall_outside_the_repo_returns_global_notes_only() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, vault) = setup(dir.path());
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let found = recall(&vault, "migration", &elsewhere, 10).unwrap();
    assert_eq!(found.context, RepoContext::NoRepo);
    assert_eq!(
        labels(&vault, &elsewhere),
        owned(&[("Migration notes are global", "unanchored")])
    );
}

#[test]
fn recall_sees_a_note_edited_and_renamed_by_hand() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    let found = recall(&vault, "migration", &app, 10).unwrap();
    let fresh = found.notes.first().unwrap();
    assert_eq!(fresh.slug, "migration-runs-migrate-once");
    let old = vault.root().join(&fresh.path);
    let text = fs::read_to_string(&old)
        .unwrap()
        .replace("The migration entry point.", "The zebra entry point.");
    fs::remove_file(&old).unwrap();
    fs::write(old.with_file_name("entry-point.md"), text).unwrap();

    let found = recall(&vault, "zebra", &app, 10).unwrap();
    let slugs: Vec<_> = found.notes.iter().map(|r| r.slug.as_str()).collect();
    assert_eq!(slugs, ["entry-point"]);
    assert_eq!(found.notes[0].freshness, Freshness::Fresh);
}

#[test]
fn recall_never_writes_the_vault_or_the_repo() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    let repo_status = git(&app, &["status", "--porcelain"]);
    let head = git(vault.root(), &["rev-parse", "HEAD"]);
    for _ in 0..20 {
        let found = recall(&vault, "migration", &app, 10).unwrap();
        assert_eq!(found.notes.len(), 4);
        assert!(found.notes.iter().any(|r| r.freshness == Freshness::Gone));
    }
    assert_eq!(git(vault.root(), &["status", "--porcelain"]), "");
    assert_eq!(git(vault.root(), &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&app, &["status", "--porcelain"]), repo_status);
}

#[test]
fn related_keeps_file_paths_in_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let (app, vault) = setup(dir.path());
    let key = RepoContext::discover(&app).key().cloned().unwrap();
    let scope = format!("repo:{}", key.as_str());
    vault
        .add(&note(
            "Source library guidelines",
            &scope,
            "active",
            &[],
            "Guidelines for src lib module.",
        ))
        .unwrap();

    let found = related(&vault, "/src/lib.rs needs a fix", &app, 10).unwrap();
    assert_eq!(found.notes.len(), 1);
    assert_eq!(found.notes[0].note.title, "Source library guidelines");

    let found_path_only = related(&vault, "/src/lib.rs", &app, 10).unwrap();
    assert_eq!(found_path_only.notes.len(), 1);
    assert_eq!(
        found_path_only.notes[0].note.title,
        "Source library guidelines"
    );
}
