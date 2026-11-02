//! The vault as a git repository: one commit per added note, idempotent adds,
//! nothing written on bad input, and concurrent writers that both land.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

use lore_memory::note::NoteError;
use lore_memory::vault::{Added, Init, Vault, VaultError};

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

const NOTE: &str = "---\ntype: howto\ntitle: Prune worktrees\n---\nRun `git worktree prune`.\n";

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
fn vault_add_commits_one_note_and_is_idempotent() {
    let (dir, vault) = new_vault();
    let (note, first) = vault.add(NOTE).unwrap();
    assert_eq!(first, Added::Committed);
    assert_eq!(note.rel_path(), "global/prune-worktrees.md");
    let (again, second) = vault.add(NOTE).unwrap();
    assert_eq!(second, Added::Unchanged);
    assert_eq!(again.id, note.id);
    assert_eq!(
        subjects(dir.path()),
        ["mem: add prune-worktrees", "mem: init vault"]
    );
    let written = vault.read("global/prune-worktrees.md").unwrap();
    assert!(written.starts_with(&format!("---\nid: {}\ntype: howto\n", note.id.as_str())));
    assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn vault_add_commits_a_note_left_uncommitted() {
    let (dir, vault) = new_vault();
    let (note, _) = vault.add(NOTE).unwrap();
    git(dir.path(), &["reset", "-q", "--soft", "HEAD~1"]);
    git(dir.path(), &["reset", "-q", "--", "global"]);
    let (_, outcome) = vault.add(&note.render()).unwrap();
    assert_eq!(outcome, Added::Committed);
    assert_eq!(
        subjects(dir.path()),
        ["mem: add prune-worktrees", "mem: init vault"]
    );
}

fn assert_nothing_written(dir: &Path) {
    assert_eq!(subjects(dir), ["mem: init vault"]);
    assert!(git(dir, &["status", "--porcelain"]).is_empty());
    assert!(!dir.join("global").exists());
}

#[test]
fn vault_add_refuses_a_bad_date_and_writes_nothing() {
    let (dir, vault) = new_vault();
    let error = vault
        .add("---\ntype: fact\ntitle: T\ncreated: 2026-02-30\n---\n")
        .unwrap_err();
    assert!(
        matches!(error, VaultError::Note(NoteError::Field { ref field, .. }) if field == "created")
    );
    assert_nothing_written(dir.path());
}

#[test]
fn vault_add_refuses_a_bad_anchor_path_and_writes_nothing() {
    let (dir, vault) = new_vault();
    let error = vault
        .add("---\ntype: fact\ntitle: T\nanchors:\n  - path: ../../etc/passwd\n---\n")
        .unwrap_err();
    assert!(
        matches!(error, VaultError::Note(NoteError::Field { ref field, .. }) if field == "anchors[0].path")
    );
    assert_nothing_written(dir.path());
}

#[test]
fn vault_add_never_overwrites_a_different_note() {
    let (dir, vault) = new_vault();
    vault.add(NOTE).unwrap();
    let before = vault.read("global/prune-worktrees.md").unwrap();
    let error = vault
        .add("---\ntype: howto\ntitle: Prune worktrees\n---\nSomething else.\n")
        .unwrap_err();
    assert!(matches!(error, VaultError::Conflict(ref rel) if rel == "global/prune-worktrees.md"));
    assert_eq!(vault.read("global/prune-worktrees.md").unwrap(), before);
    assert_eq!(subjects(dir.path()).len(), 2);
}

#[test]
fn vault_read_refuses_paths_outside_the_vault() {
    let (_dir, vault) = new_vault();
    for rel in ["../secret.md", "/etc/passwd", "global/../../x.md"] {
        assert!(
            matches!(vault.read(rel), Err(VaultError::BadPath(..))),
            "{rel}"
        );
    }
}

#[test]
fn vault_open_reports_a_missing_vault() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        Vault::open(dir.path()),
        Err(VaultError::Missing(_))
    ));
}

#[test]
fn vault_concurrent_adds_all_land_and_fsck_is_clean() {
    let (dir, _vault) = new_vault();
    let root = dir.path().to_path_buf();
    let workers: Vec<_> = (0..8)
        .map(|i| {
            let root = root.clone();
            std::thread::spawn(move || {
                let vault = Vault::open(&root).unwrap();
                let text = format!("---\ntype: fact\ntitle: Parallel note {i}\n---\nBody {i}.\n");
                vault.add(&text).unwrap().1
            })
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap(), Added::Committed);
    }
    assert_eq!(subjects(&root).len(), 9);
    for i in 0..8 {
        assert!(root.join(format!("global/parallel-note-{i}.md")).exists());
    }
    git(&root, &["fsck", "--strict"]);
    assert!(git(&root, &["status", "--porcelain"]).is_empty());
}

/// Hooks and signing configured in the vault's own git config, the way a
/// person's global config would carry them. Either one would block or fail a
/// commit if Lore inherited it.
#[test]
fn vault_commits_skip_hooks_and_signing() {
    let (dir, vault) = new_vault();
    let hooks = dir.path().join(".lore/test-hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(
        dir.path(),
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    git(dir.path(), &["config", "commit.gpgsign", "true"]);
    git(dir.path(), &["config", "gpg.program", "/nonexistent/gpg"]);
    let (_, added) = vault.add(NOTE).unwrap();
    assert_eq!(added, Added::Committed);
    assert_eq!(subjects(dir.path()).len(), 2);
}

#[test]
fn vault_lock_gives_up_with_busy_after_a_bounded_wait() {
    let (dir, vault) = new_vault();
    let held = vault.lock().unwrap();
    let started = std::time::Instant::now();
    let error = Vault::open(dir.path()).unwrap().add(NOTE).unwrap_err();
    let waited = started.elapsed();
    drop(held);
    assert!(matches!(error, VaultError::Busy(_)), "{error:?}");
    assert!(waited >= lore_memory::vault::LOCK_WAIT, "{waited:?}");
    assert!(waited < lore_memory::vault::LOCK_WAIT * 3, "{waited:?}");
    assert!(!dir.path().join("global").exists());
}
