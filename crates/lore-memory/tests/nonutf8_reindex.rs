//! FB1 finding: one non-UTF-8 file in the vault fails the whole reindex.
//!
//! `reindex` promises to rebuild the index from the vault files alone and to
//! report what it could not read. A note file edited by another tool into
//! invalid UTF-8 returns `Err(Io(..))` for the entire vault, so every valid
//! note stops being searchable and `mem reindex` exits 4 with no note named.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use lore_memory::vault::{Init, Vault, VaultError};

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
fn a_non_utf8_file_is_skipped_not_a_failed_rebuild() {
    isolate_git();
    let dir = tempfile::tempdir().unwrap();
    let (vault, init) = Vault::init(dir.path()).unwrap();
    assert_eq!(init, Init::Created);
    vault
        .add("---\ntype: fact\ntitle: Good note\n---\nfindable body\n")
        .unwrap();

    let broken = dir.path().join("global");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("bad.md"), b"---\ntype: fact\n---\n\xff\xfe\n").unwrap();
    git(dir.path(), &["add", "-A"]);
    git(
        dir.path(),
        &["commit", "-q", "-m", "a file another tool truncated"],
    );

    let reindexed = vault
        .reindex()
        .expect("one unreadable file must not fail the whole rebuild");
    assert_eq!(reindexed.indexed, 1, "{reindexed:?}");
    let skipped: Vec<(&str, String)> = reindexed
        .skipped
        .iter()
        .map(|(rel, error)| (rel.as_str(), error.to_string()))
        .collect();
    assert_eq!(
        skipped,
        [(
            "global/bad.md",
            "not a note: the file is not valid UTF-8".to_string()
        )]
    );
    assert!(
        vault
            .search("findable", 10)
            .unwrap()
            .iter()
            .any(|h| h.title.contains("Good")),
        "a valid note stopped being searchable because of a neighbour"
    );
}

#[test]
fn an_unreadable_note_file_still_fails_the_rebuild() {
    isolate_git();
    let dir = tempfile::tempdir().unwrap();
    let (vault, _) = Vault::init(dir.path()).unwrap();
    let locked = dir.path().join("global");
    fs::create_dir_all(&locked).unwrap();
    let file = locked.join("locked.md");
    fs::write(&file, "---\ntype: fact\n---\nbody\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&file).is_ok() {
        // Running as root, so permission bits do not deny the read.
        return;
    }

    let result = vault.reindex();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    match result {
        Err(VaultError::Io(what, error)) => {
            assert_eq!(what, "read global/locked.md");
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        }
        other => panic!("a permission error must still fail the rebuild: {other:?}"),
    }
}

#[test]
fn an_unreadable_note_file_still_fails_the_refresh() {
    isolate_git();
    let dir = tempfile::tempdir().unwrap();
    let (vault, _) = Vault::init(dir.path()).unwrap();
    vault
        .add("---\ntype: fact\ntitle: Good note\n---\nfindable body\n")
        .unwrap();
    let locked = dir.path().join("global");
    fs::create_dir_all(&locked).unwrap();
    let file = locked.join("locked.md");
    fs::write(&file, "---\ntype: fact\n---\nbody\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&file).is_ok() {
        // Running as root, so permission bits do not deny the read.
        return;
    }

    let result = vault.search("findable", 10);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    match result {
        Err(VaultError::Io(what, error)) => {
            assert_eq!(what, "read global/locked.md");
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        }
        other => panic!("a permission error must still fail the refresh: {other:?}"),
    }
}
