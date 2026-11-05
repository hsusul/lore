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

const ID: &str = "01JC9V6Q4M8K2T7X3N5R0B1H6D";

#[test]
fn vault_add_refuses_an_id_another_note_already_has() {
    let (dir, vault) = new_vault();
    let first = format!("---\nid: {ID}\ntype: fact\ntitle: First\n---\nOne.\n");
    vault.add(&first).unwrap();
    let second = format!("---\nid: {ID}\ntype: fact\ntitle: Second\n---\nTwo.\n");
    let error = vault.add(&second).unwrap_err();
    assert!(
        matches!(error, VaultError::DuplicateId { ref id, ref existing } if id == ID && existing == "global/first.md"),
        "{error:?}"
    );
    assert!(!dir.path().join("global/second.md").exists());
    assert_eq!(subjects(dir.path()).len(), 2);
}

#[test]
fn vault_add_refuses_an_id_from_a_hand_copied_file_the_index_has_not_seen() {
    let (dir, vault) = new_vault();
    vault.search("anything", 5).unwrap();
    let copied = format!("---\nid: {ID}\ntype: fact\ntitle: Copied\nscope: global\nstatus: active\ncreated: 2026-11-01\n---\nC.\n");
    std::fs::create_dir_all(dir.path().join("global")).unwrap();
    std::fs::write(dir.path().join("global/copied.md"), copied).unwrap();
    let error = vault
        .add(&format!("---\nid: {ID}\ntype: fact\ntitle: New\n---\nN.\n"))
        .unwrap_err();
    assert!(matches!(error, VaultError::DuplicateId { .. }), "{error:?}");
}

#[test]
fn vault_reindex_reports_duplicate_ids_with_both_paths_and_indexes_the_rest() {
    let (dir, vault) = new_vault();
    vault.add(NOTE).unwrap();
    let text = |title: &str| {
        format!("---\nid: {ID}\ntype: fact\ntitle: {title}\nscope: global\nstatus: active\ncreated: 2026-11-01\n---\nB.\n")
    };
    std::fs::write(dir.path().join("global/a.md"), text("A")).unwrap();
    std::fs::write(dir.path().join("global/b.md"), text("B")).unwrap();
    let report = vault.reindex().unwrap();
    assert_eq!(report.indexed, 3);
    assert_eq!(report.duplicates.len(), 1);
    assert_eq!(report.duplicates[0].id.as_str(), ID);
    assert_eq!(report.duplicates[0].paths, ["global/a.md", "global/b.md"]);
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

#[test]
fn vault_write_replaces_an_existing_note_in_place_and_commits() {
    let (dir, vault) = new_vault();
    let (mut note, _) = vault.add(NOTE).unwrap();
    std::fs::rename(
        dir.path().join("global/prune-worktrees.md"),
        dir.path().join("global/renamed.md"),
    )
    .unwrap();
    git(dir.path(), &["add", "-A"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "rename",
        ],
    );
    note.body = "Run `git worktree prune -v`.\n".into();
    assert_eq!(
        vault.write("global/renamed.md", &note).unwrap(),
        Added::Committed
    );
    assert_eq!(vault.read("global/renamed.md").unwrap(), note.render());
    assert_eq!(subjects(dir.path())[0], "mem: update renamed");
    assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
    assert_eq!(
        vault.write("global/renamed.md", &note).unwrap(),
        Added::Unchanged
    );
    assert_eq!(vault.search("verbose prune", 5).unwrap().len(), 0);
    assert_eq!(
        vault.search("prune", 5).unwrap()[0].path,
        "global/renamed.md"
    );
}

#[test]
fn vault_write_refuses_a_missing_file_or_another_notes_path() {
    let (_dir, vault) = new_vault();
    let (note, _) = vault.add(NOTE).unwrap();
    assert!(matches!(
        vault.write("global/nope.md", &note),
        Err(VaultError::NotFound(_))
    ));
    let (other, _) = vault
        .add("---\ntype: fact\ntitle: Other\n---\nO.\n")
        .unwrap();
    assert!(matches!(
        vault.write(&other.rel_path(), &note),
        Err(VaultError::Conflict(_))
    ));
}

#[test]
fn vault_write_temp_files_are_gitignored() {
    let (dir, _vault) = new_vault();
    std::fs::create_dir_all(dir.path().join("global")).unwrap();
    std::fs::write(dir.path().join("global/.x.md.tmp"), "half").unwrap();
    assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn vault_search_and_find_see_hand_edits_renames_and_deletes() {
    let (dir, vault) = new_vault();
    vault.add(NOTE).unwrap();
    let other = vault
        .add("---\ntype: fact\ntitle: Kingfisher facts\n---\nBlue birds.\n")
        .unwrap()
        .0;
    assert_eq!(vault.search("prune", 5).unwrap().len(), 1);
    let path = dir.path().join("global/prune-worktrees.md");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("Run", "Quetzalcoatl says run")).unwrap();
    let hits = vault.search("quetzalcoatl", 5).unwrap();
    assert_eq!(hits.len(), 1, "an edit is found without reindex");
    std::fs::rename(&path, dir.path().join("global/tidy-up.md")).unwrap();
    assert_eq!(vault.find("tidy-up").unwrap(), "global/tidy-up.md");
    assert_eq!(vault.search("quetzalcoatl", 5).unwrap()[0].slug, "tidy-up");
    assert!(matches!(
        vault.find("prune-worktrees"),
        Err(VaultError::NotFound(_))
    ));
    std::fs::remove_file(dir.path().join(other.rel_path())).unwrap();
    assert!(vault.search("kingfisher", 5).unwrap().is_empty());
    assert!(matches!(
        vault.find(other.id.as_str()),
        Err(VaultError::NotFound(_))
    ));
}

#[test]
fn vault_reindex_skips_dot_directories_and_the_inbox() {
    let (dir, vault) = new_vault();
    vault.add(NOTE).unwrap();
    std::fs::create_dir_all(dir.path().join("_inbox")).unwrap();
    std::fs::write(dir.path().join("_inbox/x.md"), "not a note").unwrap();
    std::fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
    std::fs::write(dir.path().join(".obsidian/y.md"), "not a note").unwrap();
    std::fs::write(dir.path().join("README.md"), "# my vault\n").unwrap();
    let report = vault.reindex().unwrap();
    let skipped: Vec<&str> = report.skipped.iter().map(|(rel, _)| rel.as_str()).collect();
    assert_eq!(skipped, ["README.md"]);
    assert_eq!(report.indexed, 1);
    assert_eq!(
        vault.find("prune-worktrees").unwrap(),
        "global/prune-worktrees.md"
    );
    assert_eq!(
        vault.find("prune-worktrees.md").unwrap(),
        "global/prune-worktrees.md"
    );
    assert_eq!(
        vault.find("global/prune-worktrees").unwrap(),
        "global/prune-worktrees.md"
    );
}

#[test]
fn vault_add_with_writes_what_prepare_changed_and_nothing_when_it_fails() {
    let (dir, vault) = new_vault();
    let refused = vault.add_with(NOTE, |_| Err(VaultError::NotFound("prepare".into())));
    assert!(
        matches!(refused, Err(VaultError::NotFound(_))),
        "{refused:?}"
    );
    assert_nothing_written(dir.path());

    let (note, added) = vault
        .add_with(NOTE, |note| {
            note.body.push_str("Prepared before the write.\n");
            Ok(())
        })
        .unwrap();
    assert_eq!(added, Added::Committed);
    let text = vault.read(&note.rel_path()).unwrap();
    assert!(text.ends_with("Prepared before the write.\n"), "{text}");
    assert_eq!(subjects(dir.path()).len(), 2);
}
