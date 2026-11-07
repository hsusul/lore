//! `lorectl hook` as an agent would actually invoke it.
//!
//! The contract is unusual and worth testing as a contract: a hook runs inside
//! someone's coding session, so it must never fail it. Every test here is really
//! asking the same question — does this stay out of the way?
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Instant;

/// `lorectl` with `HOME` in `homes` and a memory vault that does not exist, so
/// no real `~/Lore` is ever read.
fn lorectl_in(cwd: &Path, archive: &Path, homes: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .arg("--archive")
        .arg(archive)
        .current_dir(cwd)
        .env("HOME", homes)
        .env("LORE_VAULT_DIR", homes.join("no-vault"))
        .env("CLAUDE_CONFIG_DIR", homes.join("claude"))
        .env("CODEX_HOME", homes.join("codex"))
        .env_remove("LORE_ARCHIVE_DIR")
        .output()
        .expect("spawn lorectl")
}

/// `lorectl hook session-start` as an agent runs it, with the hook JSON on
/// stdin, `HOME` in `home`, and the vault at `vault`.
fn session_start(cwd: &Path, home: &Path, vault: &Path, stdin: &str) -> Output {
    hook("session-start", cwd, home, vault, stdin)
}

/// `lorectl hook <event>` as an agent runs it; see [`session_start`].
fn hook(event: &str, cwd: &Path, home: &Path, vault: &Path, stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["hook", event, "--archive"])
        .arg(home.join("archive"))
        .current_dir(cwd)
        .env("HOME", home)
        .env("LORE_VAULT_DIR", vault)
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("LORE_ARCHIVE_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lorectl");
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(stdin.as_bytes()).unwrap();
    drop(pipe);
    child.wait_with_output().unwrap()
}

/// `lorectl mem` against `vault`, which must succeed.
fn mem(home: &Path, vault: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("mem")
        .args(args)
        .arg("--vault")
        .arg(vault)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home)
        .env_remove("LORE_VAULT_DIR")
        .output()
        .expect("spawn lorectl");
    assert!(
        out.status.success(),
        "mem {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exited with a code")
}
fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .expect("git must be installed");
    assert!(out.status.success(), "git {args:?} failed");
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    std::fs::write(dir.path().join("seed.txt"), "seed\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "seed"]);
    dir
}

fn seed_codex_session_in(repo: &Path, homes: &Path) {
    let fixture = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../lore-core/fixtures/codex/patch_apply.jsonl"),
    )
    .unwrap();
    let retargeted = fixture.replace("\"/proj\"", &format!("{:?}", repo.display().to_string()));
    let sessions = homes.join("codex/sessions/2026/08/11");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("rollout-000000000001.jsonl"), retargeted).unwrap();
}

#[test]
fn a_hook_never_fails_the_session() {
    // Every failure a hook can plausibly meet, all of which must exit 0. A
    // non-zero exit here can stop an agent from starting, which is a far worse
    // outcome than a missing notice.
    let plain = tempfile::tempdir().unwrap();
    let homes = common::empty_homes();
    let missing = plain.path().join("no-archive");

    for args in [
        vec!["hook", "session-start"],     // no archive
        vec!["hook", "some-future-event"], // event this build never heard of
        vec!["hook", "session-start"],     // not a git repository
    ] {
        let out = lorectl_in(plain.path(), &missing, homes.path(), &args);
        assert_eq!(code(&out), 0, "{args:?} failed the session: {out:?}");
    }
}

#[test]
fn a_hook_missing_its_event_is_still_a_usage_error() {
    // The parser check runs before dispatch, so a *person* typing `lorectl hook`
    // still gets told. Only the running of a named event is forgiving.
    let plain = tempfile::tempdir().unwrap();
    let homes = common::empty_homes();
    let out = lorectl_in(plain.path(), plain.path(), homes.path(), &["hook"]);
    assert_eq!(code(&out), 1);
}

#[test]
fn a_clean_repository_produces_no_output_at_all() {
    // A hook that speaks every session is one people mute, and a muted hook is
    // worse than an uninstalled one.
    let repo = repo();
    let homes = common::empty_homes();
    let archive = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&lorectl_in(
            repo.path(),
            archive.path(),
            homes.path(),
            &["scan"]
        )),
        0
    );

    let out = lorectl_in(
        repo.path(),
        archive.path(),
        homes.path(),
        &["hook", "session-start"],
    );
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).is_empty(),
        "a clean repository should be silent, got: {}",
        stdout(&out)
    );
}

#[test]
fn at_risk_work_is_announced_once_and_names_the_next_step() {
    let repo = repo();
    let homes = common::empty_homes();
    let archive = tempfile::tempdir().unwrap();
    seed_codex_session_in(repo.path(), homes.path());
    assert_eq!(
        code(&lorectl_in(
            repo.path(),
            archive.path(),
            homes.path(),
            &["scan"]
        )),
        0
    );

    // Agent-created content reaches the index and is never committed.
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    std::fs::write(repo.path().join("src/new.ts"), "export const x = 1\n").unwrap();
    git(repo.path(), &["add", "src/new.ts"]);

    let out = lorectl_in(
        repo.path(),
        archive.path(),
        homes.path(),
        &["hook", "session-start"],
    );
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    // No vault, so the output is exactly what it was before memory notes.
    assert_eq!(
        text,
        "lore: 1 file(s) written by agents are staged and not committed \
         (a reset would discard them) \u{2014} see `lorectl status`\n"
    );
    assert!(text.contains("lore:"), "{text}");
    // The claim is only made for staged content, where a reset really would
    // discard it.
    assert!(text.contains("staged and not committed"), "{text}");
    assert!(
        text.contains("lorectl status"),
        "must name the next step: {text}"
    );
    assert_eq!(text.lines().count(), 1, "a hook gets one line: {text}");
}

#[test]
fn committing_the_work_silences_the_hook() {
    let repo = repo();
    let homes = common::empty_homes();
    let archive = tempfile::tempdir().unwrap();
    seed_codex_session_in(repo.path(), homes.path());
    assert_eq!(
        code(&lorectl_in(
            repo.path(),
            archive.path(),
            homes.path(),
            &["scan"]
        )),
        0
    );
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    std::fs::write(repo.path().join("src/new.ts"), "export const x = 1\n").unwrap();
    git(repo.path(), &["add", "src/new.ts"]);
    git(repo.path(), &["commit", "-m", "land it"]);

    let out = lorectl_in(
        repo.path(),
        archive.path(),
        homes.path(),
        &["hook", "session-start"],
    );
    assert!(
        stdout(&out).is_empty(),
        "the hook kept warning after the work landed: {}",
        stdout(&out)
    );
}

#[test]
fn a_hook_returns_promptly() {
    // It runs at session start, so its cost is paid by a human waiting. This is
    // a smoke bound, not a benchmark: it catches "accidentally scans the world",
    // which is the mistake worth catching.
    let repo = repo();
    let homes = common::empty_homes();
    let archive = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&lorectl_in(
            repo.path(),
            archive.path(),
            homes.path(),
            &["scan"]
        )),
        0
    );

    let start = Instant::now();
    let out = lorectl_in(
        repo.path(),
        archive.path(),
        homes.path(),
        &["hook", "session-start"],
    );
    let elapsed = start.elapsed();
    assert_eq!(code(&out), 0);
    assert!(
        elapsed.as_secs() < 10,
        "the hook took {elapsed:?}; a session start cannot wait that long"
    );
}

#[test]
fn a_hook_does_not_scan() {
    // Deliberate: scanning is unbounded, and making Lore the reason a session is
    // slow to start would get it uninstalled. Keeping the archive current is the
    // app's job, or an explicit `lorectl scan`.
    let repo = repo();
    let homes = common::empty_homes();
    let archive = tempfile::tempdir().unwrap();
    seed_codex_session_in(repo.path(), homes.path());

    // No scan has ever run, so the archive does not exist yet.
    let out = lorectl_in(
        repo.path(),
        archive.path(),
        homes.path(),
        &["hook", "session-start"],
    );
    assert_eq!(code(&out), 0);
    assert!(
        !archive.path().join("lore.db").exists(),
        "the hook created an archive; it must only ever read"
    );
}

const RETRY: &str = "---\nid: 01JD0000000000000000000001\ntype: pitfall\ntitle: Retry the migration\nscope: repo:81444c90f1ff44ac\nanchors:\n  - path: src/a.rs\n    lines: 1\n---\nRetry a failed migration once.\n";
const PIN: &str = "---\nid: 01JD0000000000000000000002\ntype: convention\ntitle: Pin the toolchain\nscope: repo:81444c90f1ff44ac\n---\nUse the pinned toolchain.\n";
const GLOBAL: &str = "---\nid: 01JD0000000000000000000003\ntype: fact\ntitle: Migrations are global\n---\nEvery migration is logged.\n";
const OTHER_REPO: &str = "---\nid: 01JD0000000000000000000004\ntype: fact\ntitle: Other repo\nscope: repo:aaaaaaaaaaaaaaaa\n---\nNot here.\n";

/// A home holding a vault and a repository whose origin is this project's,
/// with `src/a.rs` committed.
fn memory_home() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let vault = home.path().join("vault");
    mem(home.path(), &vault, &["init"]);
    let repo = home.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:hsusul/lore.git"],
    );
    std::fs::write(repo.join("src/a.rs"), "fn retry() {}\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "a"]);
    (home, vault, repo)
}

fn add_notes(home: &Path, vault: &Path, repo: &Path, notes: &[&str]) {
    for (i, text) in notes.iter().enumerate() {
        let file = home.join(format!("note-{i}.md"));
        std::fs::write(&file, text).unwrap();
        mem(
            home,
            vault,
            &[
                "add",
                file.to_str().unwrap(),
                "--cwd",
                repo.to_str().unwrap(),
            ],
        );
    }
}

fn cwd_json(dir: &Path) -> String {
    serde_json::json!({ "session_id": "s-1", "cwd": dir }).to_string()
}

#[test]
fn session_start_lists_the_notes_for_the_repo_named_on_stdin() {
    let (home, vault, repo) = memory_home();
    add_notes(
        home.path(),
        &vault,
        &repo,
        &[RETRY, PIN, GLOBAL, OTHER_REPO],
    );
    // The process runs elsewhere. The repository comes from the hook JSON.
    let elsewhere = tempfile::tempdir().unwrap();

    let out = session_start(elsewhere.path(), home.path(), &vault, &cwd_json(&repo));
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        "Lore memory for this repo:\n\
         - [unanchored] Pin the toolchain (01JD0000000000000000000002)\n\
         - [fresh] Retry the migration (01JD0000000000000000000001)\n\
         - [unanchored] Migrations are global (01JD0000000000000000000003)\n"
    );
}

#[test]
fn session_start_labels_a_note_whose_anchored_lines_changed() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[RETRY, GLOBAL]);
    std::fs::write(repo.join("src/a.rs"), "fn retry_twice() {}\n").unwrap();

    let out = session_start(&repo, home.path(), &vault, "");
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        "Lore memory for this repo:\n\
         - [unanchored] Migrations are global (01JD0000000000000000000003)\n\
         - [changed] Retry the migration (01JD0000000000000000000001)\n"
    );
}

#[test]
fn session_start_shows_ten_notes_and_says_how_many_it_left_out() {
    let (home, vault, repo) = memory_home();
    let notes: Vec<String> = (1..=12)
        .map(|i| {
            format!(
                "---\nid: 01JD{:022}\ntype: fact\ntitle: Note {i:02}\n---\nBody {i}.\n",
                100 + i
            )
        })
        .collect();
    let notes: Vec<&str> = notes.iter().map(String::as_str).collect();
    add_notes(home.path(), &vault, &repo, &notes);

    // No hook JSON at all, so the process directory is the repository.
    let out = session_start(&repo, home.path(), &vault, "");
    assert_eq!(code(&out), 0);
    let mut expected = String::from("Lore memory for this repo:\n");
    for i in 1..=10 {
        expected.push_str(&format!(
            "- [unanchored] Note {i:02} (01JD{:022})\n",
            100 + i
        ));
    }
    expected.push_str("- 2 more not shown. The recall_memory tool lists them.\n");
    assert_eq!(stdout(&out), expected);
}

#[test]
fn session_start_adds_nothing_without_a_vault_or_a_matching_note() {
    let (home, vault, repo) = memory_home();
    let missing = home.path().join("missing-vault");
    let not_a_vault = tempfile::tempdir().unwrap();
    for dir in [&missing, &not_a_vault.path().to_path_buf(), &vault] {
        let out = session_start(&repo, home.path(), dir, &cwd_json(&repo));
        assert_eq!(code(&out), 0);
        assert_eq!(stdout(&out), "", "vault {}", dir.display());
    }
    // A note for another repository is not this one's.
    add_notes(home.path(), &vault, &repo, &[OTHER_REPO]);
    let out = session_start(&repo, home.path(), &vault, "not json");
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "");
}

#[test]
fn at_risk_work_is_found_in_the_repo_named_on_stdin() {
    // An agent may start the hook from anywhere. The notes already follow the
    // `cwd` in the hook JSON, and the at-risk check must look at the same repo.
    let repo = repo();
    let homes = common::empty_homes();
    let archive = tempfile::tempdir().unwrap();
    seed_codex_session_in(repo.path(), homes.path());
    assert_eq!(
        code(&lorectl_in(
            repo.path(),
            archive.path(),
            homes.path(),
            &["scan"]
        )),
        0
    );
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    std::fs::write(repo.path().join("src/new.ts"), "export const x = 1\n").unwrap();
    git(repo.path(), &["add", "src/new.ts"]);

    let elsewhere = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["hook", "session-start", "--archive"])
        .arg(archive.path())
        .current_dir(elsewhere.path())
        .env("HOME", homes.path())
        .env("LORE_VAULT_DIR", homes.path().join("no-vault"))
        .env("CLAUDE_CONFIG_DIR", homes.path().join("claude"))
        .env("CODEX_HOME", homes.path().join("codex"))
        .env_remove("LORE_ARCHIVE_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lorectl");
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(cwd_json(repo.path()).as_bytes()).unwrap();
    drop(pipe);
    let out = child.wait_with_output().unwrap();

    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        "lore: 1 file(s) written by agents are staged and not committed \
         (a reset would discard them) \u{2014} see `lorectl status`\n"
    );
}

/// Scan the Codex fixture session, which ran in `repo`, into `home/archive`,
/// the archive [`session_start`] reads. Returns its archive id.
fn archive_session_in(repo: &Path, home: &Path) -> String {
    seed_codex_session_in(repo, home);
    let archive = home.join("archive");
    assert_eq!(code(&lorectl_in(repo, &archive, home, &["scan"])), 0);
    let conn = lore_core::storage::open_read_only(&archive.join("lore.db")).unwrap();
    conn.query_row("SELECT id FROM agent_session", [], |r| r.get(0))
        .unwrap()
}

/// The fixture's native Codex session id, as an agent's hook JSON carries it.
const CODEX_SESSION: &str = "019e0000-0000-7000-8000-000000000003";

#[test]
fn session_start_names_the_archived_session_from_stdin() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[GLOBAL]);
    let archive_id = archive_session_in(&repo, home.path());
    assert!(archive_id.starts_with("s_"), "{archive_id}");

    for given in [CODEX_SESSION, archive_id.as_str()] {
        let stdin = serde_json::json!({ "session_id": given, "cwd": repo }).to_string();
        let out = session_start(&repo, home.path(), &vault, &stdin);
        assert_eq!(code(&out), 0);
        assert_eq!(
            stdout(&out),
            format!(
                "Lore memory for this repo:\n\
                 - [unanchored] Migrations are global (01JD0000000000000000000003)\n\
                 Lore session: {archive_id}\n"
            ),
            "{given}"
        );
    }
}

#[test]
fn session_start_names_the_session_even_with_no_vault() {
    let (home, _vault, repo) = memory_home();
    let archive_id = archive_session_in(&repo, home.path());
    let stdin = serde_json::json!({ "session_id": CODEX_SESSION, "cwd": repo }).to_string();

    let out = session_start(&repo, home.path(), &home.path().join("no-vault"), &stdin);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), format!("Lore session: {archive_id}\n"));
}

#[test]
fn session_start_says_nothing_of_a_session_the_archive_lacks() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[GLOBAL]);
    archive_session_in(&repo, home.path());
    let stdin = serde_json::json!({ "session_id": "not-archived", "cwd": repo }).to_string();

    let out = session_start(&repo, home.path(), &vault, &stdin);
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        "Lore memory for this repo:\n\
         - [unanchored] Migrations are global (01JD0000000000000000000003)\n"
    );
}

/// Claude Code's PreToolUse hook JSON for an edit of `file_path`.
fn edit_json(cwd: &Path, file_path: &str) -> String {
    serde_json::json!({
        "session_id": "s-1",
        "cwd": cwd,
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": { "file_path": file_path, "old_string": "a", "new_string": "b" },
    })
    .to_string()
}

/// The `additionalContext` reply that carries `text` to the agent.
fn context_reply(text: &str) -> String {
    let reply = serde_json::json!({
        "hookSpecificOutput": { "hookEventName": "PreToolUse", "additionalContext": text }
    });
    format!("{reply}\n")
}

#[test]
fn pre_edit_shows_the_notes_anchored_to_the_file_as_context() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[RETRY, PIN, GLOBAL]);
    let file = repo.join("src/a.rs");
    let expected = context_reply(
        "Lore notes anchored to src/a.rs:\n\
         - [fresh] Retry the migration (01JD0000000000000000000001)\n",
    );

    let out = hook(
        "pre-edit",
        &repo,
        home.path(),
        &vault,
        &edit_json(&repo, file.to_str().unwrap()),
    );
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), expected);

    // A path relative to the session's directory names the same file.
    let src = repo.join("src");
    let out = hook(
        "pre-edit",
        &repo,
        home.path(),
        &vault,
        &edit_json(&src, "a.rs"),
    );
    assert_eq!(stdout(&out), expected);
}

#[test]
fn pre_edit_labels_a_note_whose_anchored_lines_changed() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[RETRY]);
    std::fs::write(repo.join("src/a.rs"), "fn retry_twice() {}\n").unwrap();
    let file = repo.join("src/a.rs");

    let out = hook(
        "pre-edit",
        &repo,
        home.path(),
        &vault,
        &edit_json(&repo, file.to_str().unwrap()),
    );
    assert_eq!(
        stdout(&out),
        context_reply(
            "Lore notes anchored to src/a.rs:\n\
             - [changed] Retry the migration (01JD0000000000000000000001)\n"
        )
    );
}

#[test]
fn pre_edit_is_silent_unless_a_note_is_anchored_to_the_file() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[RETRY, PIN, GLOBAL]);
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = elsewhere.path().join("a.rs");
    for stdin in [
        edit_json(&repo, repo.join("src/b.rs").to_str().unwrap()),
        edit_json(&repo, outside.to_str().unwrap()),
        serde_json::json!({ "cwd": repo, "tool_name": "Bash", "tool_input": { "command": "ls" } })
            .to_string(),
        String::new(),
        "not json".to_owned(),
    ] {
        let out = hook("pre-edit", &repo, home.path(), &vault, &stdin);
        assert_eq!(code(&out), 0, "{stdin}");
        assert_eq!(stdout(&out), "", "{stdin}");
    }
}
