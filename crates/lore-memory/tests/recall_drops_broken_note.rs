//! FB1 finding: recall drops a note it cannot parse without saying so.
//!
//! `reindex` names a file it cannot read in `skipped`. `recall` filters the same
//! failure away with `.ok()?`, so a note that was recalled yesterday disappears
//! from the result today with no label, no warning, and no way for the caller to
//! know the answer is incomplete.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::Path;
use std::process::Command;

use lore_memory::recall::recall;
use lore_memory::vault::{Init, Vault};

fn isolate_git() {
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.name=Test", "-c", "user.email=test@localhost"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
}

#[test]
fn a_note_that_stopped_parsing_is_reported_not_silently_dropped() {
    isolate_git();
    let dir = tempfile::tempdir().unwrap();
    let (vault, init) = Vault::init(dir.path()).unwrap();
    assert_eq!(init, Init::Created);
    let (note, _) = vault
        .add("---\ntype: fact\ntitle: Deploy window\n---\nthe deploy window is thursday\n")
        .unwrap();

    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    let before = recall(&vault, "deploy", repo.path(), 10).unwrap();
    assert!(
        before
            .notes
            .iter()
            .any(|n| n.note.id == note.id || n.note.title.contains("Deploy")),
        "the note was not recalled to begin with"
    );

    // A hand edit that leaves a frontmatter block without a closing fence.
    let path = dir.path().join(note.rel_path());
    fs::write(&path, "---\ntype: fact\ntitle: Deploy window\n").unwrap();
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "half a write"]);

    let after = recall(&vault, "deploy", repo.path(), 10).unwrap();
    let reindexed = vault.reindex();
    let named_in_skipped = reindexed
        .as_ref()
        .map(|r| r.skipped.iter().any(|(rel, _)| rel.contains(&note.rel_path())))
        .unwrap_or(true);
    assert!(
        named_in_skipped,
        "reindex no longer names the file, so this case is not the one under test: {reindexed:?}"
    );
    assert!(
        !after.notes.is_empty(),
        "recall silently dropped the note it cannot parse, and Recall carries no record of the \
         drop, so a caller cannot tell an incomplete answer from a complete one: {after:?}"
    );
}
