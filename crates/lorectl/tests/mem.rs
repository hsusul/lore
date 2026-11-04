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

const NOTE: &str = "---\ntype: pitfall\ntitle: Worktrees share refs\nscope: repo:3f9a1c0b5e2d7a46\nmood: calm\n---\nA fetch in one worktree is seen by all of them.\n";

#[test]
fn mem_add_twice_is_one_note_and_one_commit() {
    let (dir, vault) = init();
    let file = note_file(dir.path(), "n.md", NOTE);
    let first = lorectl(&vault, &["add", &file]);
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    assert!(stdout(&first).starts_with("added repos/3f9a1c0b5e2d7a46/worktrees-share-refs.md "));
    let second = lorectl(&vault, &["add", &file]);
    assert_eq!(code(&second), 0, "{}", stderr(&second));
    assert!(
        stdout(&second).starts_with("unchanged repos/3f9a1c0b5e2d7a46/worktrees-share-refs.md ")
    );
    assert_eq!(
        log(&vault),
        ["mem: add worktrees-share-refs", "mem: init vault"]
    );
    assert_eq!(
        std::fs::read_dir(vault.join("repos/3f9a1c0b5e2d7a46"))
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
        std::fs::read_to_string(vault.join("repos/3f9a1c0b5e2d7a46/worktrees-share-refs.md"))
            .unwrap()
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
        "worktrees-share-refs  repo:3f9a1c0b5e2d7a46  A fetch in one worktree is seen by all of them.\n"
    );
    let none = lorectl(&vault, &["search", "kingfisher"]);
    assert_eq!((code(&none), stdout(&none).as_str()), (0, ""));
}

fn assert_refused(vault: &Path, out: &Output, field: &str) {
    assert_eq!(code(out), 7, "{}", stderr(out));
    assert!(stderr(out).contains(field), "{}", stderr(out));
    assert_eq!(log(vault), ["mem: init vault"]);
    assert_eq!(status(vault), "");
    assert!(!vault.join("global").exists());
}

#[test]
fn mem_add_refuses_a_malformed_date_with_exit_7() {
    let (dir, vault) = init();
    let text = "---\ntype: fact\ntitle: T\ncreated: 2026-11-31\n---\n";
    let out = lorectl(&vault, &["add", &note_file(dir.path(), "n.md", text)]);
    assert_refused(&vault, &out, "`created`");
}

#[test]
fn mem_add_refuses_a_bad_anchor_path_with_exit_7() {
    let (dir, vault) = init();
    for path in ["/etc/passwd", "../outside.rs"] {
        let text = format!("---\ntype: fact\ntitle: T\nanchors:\n  - path: {path}\n---\n");
        let out = lorectl(&vault, &["add", &note_file(dir.path(), "n.md", &text)]);
        assert_refused(&vault, &out, "`anchors[0].path`");
    }
}

#[test]
fn mem_add_cwd_refuses_an_anchor_it_cannot_pin_with_exit_7() {
    let (dir, vault) = init();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("a.rs"), "one line\n").unwrap();
    let cwd = repo.to_str().unwrap();
    for anchor in ["path: gone.rs", "path: a.rs\n    lines: 2-3"] {
        let text = format!("---\ntype: fact\ntitle: T\nanchors:\n  - {anchor}\n---\n");
        let file = note_file(dir.path(), "n.md", &text);
        let out = lorectl(&vault, &["add", &file, "--cwd", cwd]);
        assert_refused(&vault, &out, "anchor `");
    }
}

#[test]
fn mem_show_refuses_a_path_outside_the_vault_with_exit_7() {
    let (_dir, vault) = init();
    let out = lorectl(&vault, &["show", "../secret.md"]);
    assert_eq!(code(&out), 7, "{}", stderr(&out));
}

const DUP: &str = "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H6D\ntype: fact\ntitle: First\n---\nOne.\n";

#[test]
fn mem_add_refuses_an_id_in_use_with_exit_7() {
    let (dir, vault) = init();
    let first = lorectl(&vault, &["add", &note_file(dir.path(), "a.md", DUP)]);
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    let second = DUP.replace("First", "Second");
    let out = lorectl(&vault, &["add", &note_file(dir.path(), "b.md", &second)]);
    assert_eq!(code(&out), 7, "{}", stderr(&out));
    assert!(stderr(&out).contains("global/first.md"), "{}", stderr(&out));
    assert_eq!(log(&vault).len(), 2);
    assert!(!vault.join("global/second.md").exists());
}

#[test]
fn mem_reindex_names_both_paths_of_a_duplicate_id_and_exits_7() {
    let (dir, vault) = init();
    lorectl(&vault, &["add", &note_file(dir.path(), "a.md", DUP)]);
    let text = std::fs::read_to_string(vault.join("global/first.md")).unwrap();
    std::fs::write(vault.join("global/copy-of-first.md"), text).unwrap();
    let out = lorectl(&vault, &["reindex"]);
    assert_eq!(code(&out), 7, "{}", stderr(&out));
    assert_eq!(stdout(&out), "indexed 2 notes, skipped 0 files\n");
    let err = stderr(&out);
    assert!(
        err.contains("01JC9V6Q4M8K2T7X3N5R0B1H6D")
            && err.contains("global/copy-of-first.md")
            && err.contains("global/first.md"),
        "{err}"
    );
    let shown = lorectl(&vault, &["show", "copy-of-first"]);
    assert_eq!(code(&shown), 0, "{}", stderr(&shown));
}

#[test]
fn mem_help_prints_only_the_mem_section() {
    for args in [
        &["mem", "--help"][..],
        &["help", "mem"][..],
        &["mem", "add", "--help"][..],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
        let text = stdout(&out);
        assert!(text.starts_with("Memory (lorectl mem"), "{args:?}: {text}");
        assert!(!text.contains("Exit codes") || text.contains("7"), "{text}");
        assert!(!text.contains("--archive"), "{args:?}: {text}");
    }
    let full = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("help")
        .output()
        .unwrap();
    assert_eq!(code(&full), 0);
    assert!(stdout(&full).contains("--archive"));
}

#[test]
fn mem_search_sees_a_hand_edit_without_reindex() {
    let (dir, vault) = init();
    lorectl(&vault, &["add", &note_file(dir.path(), "n.md", NOTE)]);
    assert_eq!(code(&lorectl(&vault, &["search", "fetch"])), 0);
    let path = vault.join("repos/3f9a1c0b5e2d7a46/worktrees-share-refs.md");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("A fetch", "A quetzalcoatl fetch")).unwrap();
    let out = lorectl(&vault, &["search", "quetzalcoatl"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("worktrees-share-refs "),
        "{}",
        stdout(&out)
    );
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

const REPO_NOTE: &str = "---\ntype: pitfall\ntitle: Retry the migration\nscope: repo:81444c90f1ff44ac\nanchors:\n  - path: src/a.rs\n    lines: 1\n---\nRetry a failed migration once.\n";

/// A repository whose origin is this project's, with `src/a.rs` committed.
fn lore_repo(dir: &Path) -> std::path::PathBuf {
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:hsusul/lore.git"],
    );
    std::fs::write(repo.join("src/a.rs"), "fn retry() {}\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "a"]);
    repo
}

#[test]
fn mem_add_cwd_pins_anchors_and_names_the_repo() {
    let (dir, vault) = init();
    let repo = lore_repo(dir.path());
    let file = note_file(dir.path(), "r.md", REPO_NOTE);
    let added = lorectl(&vault, &["add", &file, "--cwd", repo.to_str().unwrap()]);
    assert_eq!(code(&added), 0, "{}", stderr(&added));
    let stored =
        std::fs::read_to_string(vault.join("repos/lore-81444c90f1ff44ac/retry-the-migration.md"))
            .unwrap();
    assert!(stored.contains("    hash: b3:"), "{stored}");
    assert!(
        stored.contains("\nscope: repo:81444c90f1ff44ac\nrepo: lore\n"),
        "{stored}"
    );
    assert_eq!(status(&vault), "");
}

#[test]
fn mem_recall_labels_notes_against_the_repo_at_cwd() {
    let (dir, vault) = init();
    let repo = lore_repo(dir.path());
    let global_note =
        "---\ntype: fact\ntitle: Migrations are global\n---\nEvery migration is logged.\n";
    let cwd = repo.to_str().unwrap();
    let file = note_file(dir.path(), "r.md", REPO_NOTE);
    let added = lorectl(&vault, &["add", &file, "--cwd", cwd]);
    assert_eq!(code(&added), 0, "{}", stderr(&added));
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
fn mem_recall_without_a_query_prints_every_note_that_applies() {
    let (dir, vault) = init();
    let repo = lore_repo(dir.path());
    let cwd = repo.to_str().unwrap();
    let notes = [
        ("r.md", REPO_NOTE),
        (
            "g.md",
            "---\ntype: fact\ntitle: Migrations are global\n---\nEvery migration is logged.\n",
        ),
        (
            "l.md",
            "---\ntype: fact\ntitle: Logs rotate\n---\none two three four five six seven eight nine ten eleven twelve thirteen\n",
        ),
        (
            "o.md",
            "---\ntype: fact\ntitle: Other repo\nscope: repo:aaaaaaaaaaaaaaaa\n---\nNot here.\n",
        ),
        (
            "x.md",
            "---\ntype: fact\ntitle: Retired\nstatus: retired\n---\nGone for good.\n",
        ),
    ];
    for (name, text) in notes {
        let added = lorectl(
            &vault,
            &["add", &note_file(dir.path(), name, text), "--cwd", cwd],
        );
        assert_eq!(code(&added), 0, "{}", stderr(&added));
    }

    let out = lorectl(&vault, &["recall", "--cwd", cwd]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "unanchored  logs-rotate            global                 one two three four five six seven eight nine ten eleven twelve...\n\
         unanchored  migrations-are-global  global                 Every migration is logged.\n\
         fresh       retry-the-migration    repo:81444c90f1ff44ac  Retry a failed migration once.\n"
    );
    assert_eq!(stderr(&out), "");

    let out = lorectl(&vault, &["recall", "--limit", "1", "--cwd", cwd]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "unanchored  logs-rotate  global  one two three four five six seven eight nine ten eleven twelve...\n"
    );
    assert_eq!(status(&vault), "");
}

#[test]
fn mem_recall_labels_anchors_it_cannot_check_as_unchecked() {
    let (dir, vault) = init();
    let anchored = "---\ntype: fact\ntitle: Migrations keep a log\n\
anchors:\n  - path: src/db.rs\n    lines: 2-3\n    hash: b3:0123456789abcdef\n\
---\nEvery migration writes a log line.\n";
    let added = lorectl(&vault, &["add", &note_file(dir.path(), "a.md", anchored)]);
    assert_eq!(code(&added), 0, "{}", stderr(&added));
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let cwd = elsewhere.to_str().unwrap();

    let out = lorectl(&vault, &["recall", "migration", "--cwd", cwd]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "unchecked   migrations-keep-a-log  global  Every migration writes a log line.\n"
    );

    let out = lorectl(&vault, &["recall", "migration", "--json", "--cwd", cwd]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(json["notes"][0]["freshness"], "unchecked");
    assert_eq!(json["notes"][0]["anchors"], serde_json::json!([]));
}

#[test]
fn mem_recall_names_a_note_it_cannot_parse() {
    let (dir, vault) = init();
    let deploy = "---\ntype: fact\ntitle: Deploy window\n---\nthe deploy window is thursday\n";
    let added = lorectl(&vault, &["add", &note_file(dir.path(), "d.md", deploy)]);
    assert_eq!(code(&added), 0, "{}", stderr(&added));
    std::fs::write(
        vault.join("global/deploy-window.md"),
        "---\ntype: fact\ntitle: Deploy window\n",
    )
    .unwrap();
    let cwd = dir.path().to_str().unwrap();

    let out = lorectl(&vault, &["recall", "deploy", "--cwd", cwd]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(
        stderr(&out).contains(
            "lorectl: skipped global/deploy-window.md: not a note: the frontmatter has no closing `---` line\n"
        ),
        "{}",
        stderr(&out)
    );

    let out = lorectl(&vault, &["recall", "deploy", "--json", "--cwd", cwd]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(json["notes"], serde_json::json!([]));
    assert_eq!(
        json["skipped"],
        serde_json::json!([{
            "path": "global/deploy-window.md",
            "error": "not a note: the frontmatter has no closing `---` line",
        }])
    );
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

#[test]
fn mem_add_from_inside_a_git_hook_commits_to_the_vault() {
    // A git hook runs with GIT_DIR and GIT_INDEX_FILE set to the repo being
    // committed to. A note added from one must still land in the vault.
    let (dir, vault) = init();
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&other)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "-c",
        "user.name=T",
        "-c",
        "user.email=t@example.invalid",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "other",
    ]);

    let file = note_file(dir.path(), "n.md", NOTE);
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["mem", "add", &file, "--vault"])
        .arg(&vault)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir.path())
        .env("GIT_DIR", other.join(".git"))
        .env("GIT_INDEX_FILE", other.join(".git/index"))
        .env_remove("LORE_VAULT_DIR")
        .output()
        .unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    assert_eq!(
        log(&vault),
        ["mem: add worktrees-share-refs", "mem: init vault"]
    );
    assert_eq!(log(&other), ["other"], "the hook's repo gained a commit");
    assert_eq!(status(&vault), "");
}

#[test]
fn the_global_json_flag_reaches_recall_and_eval() {
    // `lorectl --json mem inbox` and `lorectl --json mem mine` print JSON, so
    // recall and eval must too, not only with the flag after the subcommand.
    let (dir, vault) = init();
    for sub in ["recall", "eval"] {
        let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
            .args(["--json", "mem", sub, "--vault"])
            .arg(&vault)
            .current_dir(dir.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", dir.path())
            .env_remove("LORE_VAULT_DIR")
            .output()
            .unwrap();
        assert_eq!(code(&out), 0, "{sub}: {}", stderr(&out));
        let text = stdout(&out);
        assert!(
            serde_json::from_str::<serde_json::Value>(&text).is_ok(),
            "`lorectl --json mem {sub}` printed: {text}"
        );
    }
}
