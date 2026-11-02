//! The vault as a git repository: one commit per added note, idempotent adds,
//! nothing written on bad input, and concurrent writers that both land.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

use lore_memory::vault::{Init, Vault, VaultError};

/// Run git, and the vault's own git calls, without the developer's config, so
/// the identity fallback is what CI sees.
fn isolate_git() {
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn subjects(root: &Path) -> Vec<String> {
    git(root, &["log", "--format=%s"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn new_vault() -> (tempfile::TempDir, Vault) {
    isolate_git();
    let dir = tempfile::tempdir().unwrap();
    let (vault, init) = Vault::init(dir.path()).unwrap();
    assert_eq!(init, Init::Created);
    (dir, vault)
}

#[test]
fn vault_init_is_a_git_repo_that_ignores_lore_and_inbox() {
    let (dir, _vault) = new_vault();
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
        ".lore/\n_inbox/\n.*.md.tmp\n"
    );
    assert_eq!(subjects(dir.path()), ["mem: init vault"]);
    let (_, again) = Vault::init(dir.path()).unwrap();
    assert_eq!(again, Init::AlreadyInitialized);
    assert_eq!(subjects(dir.path()), ["mem: init vault"]);
}

#[test]
fn vault_open_reports_a_missing_vault() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        Vault::open(dir.path()),
        Err(VaultError::Missing(_))
    ));
}
