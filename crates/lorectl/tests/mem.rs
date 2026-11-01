//! `lorectl mem` as a person runs it: exit codes, idempotent adds, and nothing
//! written when a note is refused.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

fn lorectl(vault: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("mem")
        .args(args)
        .arg("--vault")
        .arg(vault)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", vault.parent().unwrap())
        .env_remove("LORE_VAULT_DIR")
        .output()
        .expect("spawn lorectl")
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exited with a code")
}
fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn log(vault: &Path) -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(["log", "--format=%s"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn status(vault: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn init() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    let out = lorectl(&vault, &["init"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    (dir, vault)
}

fn note_file(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_string_lossy().into_owned()
}

const NOTE: &str = "---\ntype: pitfall\ntitle: Worktrees share refs\nscope: repo:3f9a1c0b\nmood: calm\n---\nA fetch in one worktree is seen by all of them.\n";

#[test]
fn mem_add_twice_is_one_note_and_one_commit() {
    let (dir, vault) = init();
    let file = note_file(dir.path(), "n.md", NOTE);
    let first = lorectl(&vault, &["add", &file]);
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    assert!(stdout(&first).starts_with("added repos/3f9a1c0b/worktrees-share-refs.md "));
    let second = lorectl(&vault, &["add", &file]);
    assert_eq!(code(&second), 0, "{}", stderr(&second));
    assert!(stdout(&second).starts_with("unchanged repos/3f9a1c0b/worktrees-share-refs.md "));
    assert_eq!(
        log(&vault),
        ["mem: add worktrees-share-refs", "mem: init vault"]
    );
    assert_eq!(
        std::fs::read_dir(vault.join("repos/3f9a1c0b"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn mem_show_prints_the_file_with_unknown_keys() {
    let (dir, vault) = init();
    let file = note_file(dir.path(), "n.md", NOTE);
    lorectl(&vault, &["add", &file]);
    let shown = lorectl(&vault, &["show", "worktrees-share-refs"]);
    assert_eq!(code(&shown), 0, "{}", stderr(&shown));
    let text = stdout(&shown);
    assert!(text.contains("\nmood: calm\n"), "{text}");
    assert_eq!(
        text,
        std::fs::read_to_string(vault.join("repos/3f9a1c0b/worktrees-share-refs.md")).unwrap()
    );
}

#[test]
fn mem_search_prints_slug_scope_and_excerpt() {
    let (dir, vault) = init();
    lorectl(&vault, &["add", &note_file(dir.path(), "n.md", NOTE)]);
    let out = lorectl(&vault, &["search", "fetch worktree"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "worktrees-share-refs  repo:3f9a1c0b  A fetch in one worktree is seen by all of them.\n"
    );
    let none = lorectl(&vault, &["search", "kingfisher"]);
    assert_eq!((code(&none), stdout(&none).as_str()), (0, ""));
}

fn assert_refused(vault: &Path, out: &Output, field: &str) {
    assert_eq!(code(out), 2, "{}", stderr(out));
    assert!(stderr(out).contains(field), "{}", stderr(out));
    assert_eq!(log(vault), ["mem: init vault"]);
    assert_eq!(status(vault), "");
    assert!(!vault.join("global").exists());
}

#[test]
fn mem_add_refuses_a_malformed_date_with_exit_2() {
    let (dir, vault) = init();
    let text = "---\ntype: fact\ntitle: T\ncreated: 2026-11-31\n---\n";
    let out = lorectl(&vault, &["add", &note_file(dir.path(), "n.md", text)]);
    assert_refused(&vault, &out, "`created`");
}

#[test]
fn mem_add_refuses_a_bad_anchor_path_with_exit_2() {
    let (dir, vault) = init();
    for path in ["/etc/passwd", "../outside.rs"] {
        let text = format!("---\ntype: fact\ntitle: T\nanchors:\n  - path: {path}\n---\n");
        let out = lorectl(&vault, &["add", &note_file(dir.path(), "n.md", &text)]);
        assert_refused(&vault, &out, "`anchors[0].path`");
    }
}

#[test]
fn mem_show_refuses_a_path_outside_the_vault_with_exit_2() {
    let (_dir, vault) = init();
    let out = lorectl(&vault, &["show", "../secret.md"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
}

#[test]
fn mem_without_a_vault_says_how_to_make_one() {
    let dir = tempfile::tempdir().unwrap();
    let out = lorectl(&dir.path().join("none"), &["search", "x"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("lorectl mem init"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn mem_rejects_a_relative_vault_as_usage() {
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["mem", "init", "--vault", "relative/vault"])
        .output()
        .unwrap();
    assert_eq!(code(&out), 1, "{}", stderr(&out));
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=T", "-c", "user.email=t@localhost"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn mem_recall_labels_notes_against_the_repo_at_cwd() {
    let (dir, vault) = init();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:hsusul/lore.git"],
    );
    std::fs::write(repo.join("src/a.rs"), "fn retry() {}\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "a"]);
    let repo_note = "---\ntype: pitfall\ntitle: Retry the migration\nscope: repo:81444c90f1ff44ac\nanchors:\n  - path: src/a.rs\n    lines: 1\n---\nRetry a failed migration once.\n";
    let global_note =
        "---\ntype: fact\ntitle: Migrations are global\n---\nEvery migration is logged.\n";
    let cwd = repo.to_str().unwrap();
    let added = lorectl(
        &vault,
        &[
            "add",
            &note_file(dir.path(), "r.md", repo_note),
            "--cwd",
            cwd,
        ],
    );
    assert_eq!(code(&added), 0, "{}", stderr(&added));
    let stored =
        std::fs::read_to_string(vault.join("repos/81444c90f1ff44ac/retry-the-migration.md"))
            .unwrap();
    assert!(stored.contains("    hash: b3:"), "{stored}");
    lorectl(
        &vault,
        &["add", &note_file(dir.path(), "g.md", global_note)],
    );

    std::fs::write(repo.join("src/a.rs"), "fn retry_twice() {}\n").unwrap();
    let out = lorectl(&vault, &["recall", "migration", "--cwd", cwd]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "unanchored  migrations-are-global  global                 Every migration is logged.\n\
         changed     retry-the-migration    repo:81444c90f1ff44ac  Retry a failed migration once.\n"
    );
    assert_eq!(stderr(&out), "");

    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let out = lorectl(
        &vault,
        &[
            "recall",
            "migration",
            "--json",
            "--cwd",
            elsewhere.to_str().unwrap(),
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stderr(&out).matches("recalling global notes only").count(),
        1
    );
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(json["repo"]["state"], "no_repo");
    assert_eq!(json["notes"].as_array().unwrap().len(), 1);
    assert_eq!(json["notes"][0]["freshness"], "unanchored");
    assert_eq!(status(&vault), "");
}

#[test]
fn mem_reindex_reports_skipped_files() {
    let (dir, vault) = init();
    lorectl(&vault, &["add", &note_file(dir.path(), "n.md", NOTE)]);
    std::fs::write(vault.join("README.md"), "# vault\n").unwrap();
    let out = lorectl(&vault, &["reindex"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "indexed 1 notes, skipped 1 files\n");
    assert!(
        stderr(&out).contains("skipped README.md: not a note"),
        "{}",
        stderr(&out)
    );
}
