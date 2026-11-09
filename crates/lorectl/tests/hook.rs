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

/// `lorectl mem` against `vault` as a person runs it, at a terminal, which
/// must succeed.
fn mem(home: &Path, vault: &Path, args: &[&str]) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lorectl"));
    command
        .arg("mem")
        .args(args)
        .arg("--vault")
        .arg(vault)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home)
        .env_remove("LORE_VAULT_DIR");
    let out = common::on_a_terminal(&mut command, "");
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
         Lore memory for all repos:\n\
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
         - [changed] Retry the migration (01JD0000000000000000000001)\n\
         Lore memory for all repos:\n\
         - [unanchored] Migrations are global (01JD0000000000000000000003)\n"
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
    let mut expected = String::from("Lore memory for all repos:\n");
    for i in 1..=10 {
        expected.push_str(&format!(
            "- [unanchored] Note {i:02} (01JD{:022})\n",
            100 + i
        ));
    }
    expected.push_str("- 2 more not shown. The recall_memory tool lists them.\n");
    assert_eq!(stdout(&out), expected);
}

/// A note with id number `n`, `title`, and this project's repository scope,
/// or global scope when `repo` is false.
fn titled(n: usize, title: &str, repo: bool) -> String {
    let scope = if repo {
        "scope: repo:81444c90f1ff44ac\n"
    } else {
        ""
    };
    format!("---\nid: 01JD{n:022}\ntype: fact\ntitle: {title}\n{scope}---\nBody {n}.\n")
}

/// The titles of a notes section, in order, without the trailing line.
fn titles_in(section: &str) -> Vec<String> {
    section
        .lines()
        .filter_map(|l| l.strip_prefix("- ["))
        .filter_map(|l| l.split_once("] "))
        .map(|(_, rest)| rest.rsplit_once(" (").map_or(rest, |(t, _)| t).to_owned())
        .collect()
}

/// More notes than fit: each session start favors the ones earlier sessions
/// were not shown, so within a few sessions every note reaches an agent.
#[test]
fn session_start_rotates_notes_that_do_not_all_fit() {
    let (home, vault, repo) = memory_home();
    let mut notes: Vec<String> = (1..=12)
        .map(|i| titled(500 + i, &format!("Repo lesson {i:02}"), true))
        .collect();
    notes.extend((1..=10).map(|i| titled(600 + i, &format!("Global rule {i:02}"), false)));
    // Anchored to a file that is then deleted: gone, so it never takes a
    // fresh note's slot however rarely it was shown.
    notes.push(RETRY.to_owned());
    let notes: Vec<&str> = notes.iter().map(String::as_str).collect();
    add_notes(home.path(), &vault, &repo, &notes);
    std::fs::remove_file(repo.join("src/a.rs")).unwrap();

    let mut seen = std::collections::BTreeSet::new();
    for session in ["s-rot-1", "s-rot-2", "s-rot-3"] {
        let stdin = serde_json::json!({ "session_id": session, "cwd": repo }).to_string();
        let out = stdout(&session_start(&repo, home.path(), &vault, &stdin));
        let titles = titles_in(&out);
        assert_eq!(titles.len(), 10, "{out}");
        assert!(
            titles.iter().filter(|t| t.starts_with("Global")).count() >= 4,
            "{out}"
        );
        seen.extend(titles);
    }
    assert!(!seen.contains("Retry the migration"), "{seen:?}");
    assert_eq!(seen.len(), 22, "{seen:?}");
}

#[test]
fn session_start_keeps_slots_for_global_notes_in_a_repo_with_many_notes() {
    let (home, vault, repo) = memory_home();
    let mut notes: Vec<String> = (1..=12)
        .map(|i| titled(200 + i, &format!("Repo lesson {i:02}"), true))
        .collect();
    notes.extend((1..=6).map(|i| titled(300 + i, &format!("Global rule {i:02}"), false)));
    // The same global rule written twice is one note, counted once.
    notes.push(titled(399, "The global rule 01", false));
    let notes: Vec<&str> = notes.iter().map(String::as_str).collect();
    add_notes(home.path(), &vault, &repo, &notes);

    let out = stdout(&session_start(&repo, home.path(), &vault, ""));
    let titles = titles_in(&out);
    let repo_shown = titles.iter().filter(|t| t.starts_with("Repo")).count();
    let global_shown = titles.iter().filter(|t| t.contains("lobal rule")).count();
    assert_eq!((repo_shown, global_shown), (6, 4), "{out}");
    assert!(
        out.ends_with("- 8 more not shown. The recall_memory tool lists them.\n"),
        "{out}"
    );
}

#[test]
fn session_start_in_a_repo_with_only_its_own_notes_shows_ten_of_them() {
    let (home, vault, repo) = memory_home();
    let notes: Vec<String> = (1..=12)
        .map(|i| titled(200 + i, &format!("Repo lesson {i:02}"), true))
        .collect();
    let notes: Vec<&str> = notes.iter().map(String::as_str).collect();
    add_notes(home.path(), &vault, &repo, &notes);

    let out = stdout(&session_start(&repo, home.path(), &vault, ""));
    let expected: Vec<String> = (1..=10).map(|i| format!("Repo lesson {i:02}")).collect();
    assert_eq!(titles_in(&out), expected, "{out}");
    assert!(out.ends_with("- 2 more not shown. The recall_memory tool lists them.\n"));
}

#[test]
fn session_start_shows_a_lesson_written_twice_once() {
    let (home, vault, repo) = memory_home();
    let follow_up = [
        "List follow-up items in chat; don't file them to a team tracker",
        "List follow-up items in chat instead of filing them to a team tracker",
    ];
    let threads = [
        "Resolve Greptile threads you fix; reply then resolve the ones you decline",
        "Resolve review threads once addressed; reply, then resolve, the ones you decline",
    ];
    let notes = [
        titled(401, follow_up[0], true),
        titled(402, follow_up[1], true),
        titled(403, threads[0], true),
        titled(404, threads[1], true),
        titled(405, "Never add AI attribution", false),
    ];
    let notes: Vec<&str> = notes.iter().map(String::as_str).collect();
    add_notes(home.path(), &vault, &repo, &notes);

    let out = stdout(&session_start(&repo, home.path(), &vault, ""));
    let titles = titles_in(&out);
    let follow_ups = titles.iter().filter(|t| follow_up.contains(&t.as_str()));
    assert_eq!(follow_ups.count(), 1, "{out}");
    // Related but distinct lessons both show.
    assert!(
        threads.iter().all(|t| titles.iter().any(|s| s == t)),
        "{out}"
    );
    assert!(
        titles.iter().any(|t| t == "Never add AI attribution"),
        "{out}"
    );
    assert_eq!(titles.len(), 4, "{out}");
    assert!(!out.contains("more not shown"), "{out}");
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
fn session_start_uses_general_header_outside_a_git_repository() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[GLOBAL]);
    let plain = tempfile::tempdir().unwrap();

    let out = session_start(plain.path(), home.path(), &vault, "");
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        "Lore memory:\n\
         - [unanchored] Migrations are global (01JD0000000000000000000003)\n"
    );
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
                "Lore memory for all repos:\n\
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
        "Lore memory for all repos:\n\
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

/// The UserPromptSubmit hook JSON for `prompt` in `session`.
fn prompt_json(cwd: &Path, session: &str, prompt: &str) -> String {
    serde_json::json!({
        "session_id": session,
        "cwd": cwd,
        "hook_event_name": "UserPromptSubmit",
        "prompt": prompt,
    })
    .to_string()
}

/// The UserPromptSubmit `additionalContext` reply that carries `text`.
fn prompt_reply(text: &str) -> String {
    let reply = serde_json::json!({
        "hookSpecificOutput": { "hookEventName": "UserPromptSubmit", "additionalContext": text }
    });
    format!("{reply}\n")
}

const RETRY_FOR_PROMPT: &str = "Lore notes for this request:\n\
     - [fresh] Retry the migration (01JD0000000000000000000001)\n";

#[test]
fn prompt_shows_the_note_that_shares_words_with_the_prompt() {
    let (home, vault, repo) = memory_home();
    add_notes(
        home.path(),
        &vault,
        &repo,
        &[RETRY, PIN, GLOBAL, OTHER_REPO],
    );
    // GLOBAL shares only "migration"; one shared word is not enough.
    let stdin = prompt_json(&repo, "s-1", "Why did the migration fail? Retry it.");

    let out = hook("prompt", &repo, home.path(), &vault, &stdin);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), prompt_reply(RETRY_FOR_PROMPT));
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        reply["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
}

#[test]
fn prompt_is_silent_without_matching_words() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[RETRY, PIN, GLOBAL]);
    for prompt in [
        "deploy the frontend bundle",
        "yes",
        "ok, continue",
        "/compact",
        "",
    ] {
        let stdin = prompt_json(&repo, "s-1", prompt);
        let out = hook("prompt", &repo, home.path(), &vault, &stdin);
        assert_eq!(code(&out), 0, "{prompt}");
        assert_eq!(stdout(&out), "", "{prompt}");
    }
    let out = hook("prompt", &repo, home.path(), &vault, "not json");
    assert_eq!((code(&out), stdout(&out)), (0, String::new()));
}

/// A background task's result, as Claude Code injects it as a user turn.
/// Its words match RETRY, so only the skip keeps it silent.
const TASK_NOTIFICATION: &str = "<task-notification>\n<task-id>a1b2</task-id>\n\
     <status>completed</status>\n<summary>Retry the migration agent finished</summary>\n\
     <result>Why did the migration fail? Retry it.</result>\n</task-notification>";

#[test]
fn prompt_is_silent_for_a_turn_the_host_wrote() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[RETRY, PIN, GLOBAL]);
    for prompt in [
        TASK_NOTIFICATION.to_owned(),
        format!("  {TASK_NOTIFICATION}\n<system-reminder>retry the migration</system-reminder>\n"),
        "I hit my usage limit while you were working; retry the migration now.".to_owned(),
    ] {
        let stdin = prompt_json(&repo, "s-1", &prompt);
        let out = hook("prompt", &repo, home.path(), &vault, &stdin);
        assert_eq!(code(&out), 0, "{prompt}");
        assert_eq!(stdout(&out), "", "{prompt}");
    }
}

#[test]
fn prompt_recalls_on_the_typed_words_beside_a_notification() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[RETRY, PIN, GLOBAL]);
    let typed = format!("{TASK_NOTIFICATION}\nWhy did the migration fail? Retry it.");
    let stdin = prompt_json(&repo, "s-1", &typed);
    let out = hook("prompt", &repo, home.path(), &vault, &stdin);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), prompt_reply(RETRY_FOR_PROMPT));
}

const PLANNING: &str = "---\nid: 01JD0000000000000000000005\ntype: convention\ntitle: Keep planning docs local\n---\nDon't commit planning docs. They aren't for the repo.\n";

const PLANNING_FOR_PROMPT: &str = "Lore notes for this request:\n\
     - [unanchored] Keep planning docs local (01JD0000000000000000000005)\n";

#[test]
fn prompt_does_not_match_a_note_on_a_shared_contraction() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[PLANNING]);
    // "don't" once split into "don" and "t", two words every such note shares.
    for prompt in ["don't add a changelog", "don\u{2019}t add a changelog"] {
        let stdin = prompt_json(&repo, "s-1", prompt);
        let out = hook("prompt", &repo, home.path(), &vault, &stdin);
        assert_eq!(code(&out), 0, "{prompt}");
        assert_eq!(stdout(&out), "", "{prompt}");
    }
    // Two real shared words still match, with either apostrophe.
    for (session, prompt) in [
        ("s-2", "don't commit the planning notes"),
        ("s-3", "don\u{2019}t commit the planning notes"),
    ] {
        let stdin = prompt_json(&repo, session, prompt);
        let out = hook("prompt", &repo, home.path(), &vault, &stdin);
        assert_eq!(stdout(&out), prompt_reply(PLANNING_FOR_PROMPT), "{prompt}");
    }
}

/// A note whose lesson is about planning docs, with a receipt that quotes the
/// person saying something about a release.
const QUOTED: &str = "---\nid: 01JD0000000000000000000010\ntype: convention\ntitle: Keep planning docs local\n---\nThe agent committed planning docs, and the person wanted them kept local.\n\nReceipts:\n- \"stop, ship the release first, and keep planning docs local\" (codex, session s9, 2026-01-02)\n";

const QUOTED_FOR_PROMPT: &str = "Lore notes for this request:\n\
     - [unanchored] Keep planning docs local (01JD0000000000000000000010)\n";

#[test]
fn prompt_does_not_match_on_quoted_receipts_alone() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[QUOTED]);
    // "ship" and "release" are only in what the person once said.
    let stdin = prompt_json(&repo, "s-1", "ship the release");
    let out = hook("prompt", &repo, home.path(), &vault, &stdin);
    assert_eq!((code(&out), stdout(&out)), (0, String::new()));
    // With a word of the lesson itself, a quoted word can be the second one.
    let stdin = prompt_json(&repo, "s-2", "ship the planning work");
    let out = hook("prompt", &repo, home.path(), &vault, &stdin);
    assert_eq!(stdout(&out), prompt_reply(QUOTED_FOR_PROMPT));
}

/// The `n`th of a set of notes that all hold "deploy" and "server".
fn deploy_note(n: usize) -> String {
    format!(
        "---\nid: 01JD00000000000000000000{n:02}\ntype: convention\ntitle: Deploy rule {n}\n---\n\
         Before you deploy to a server, check item {n}.\n"
    )
}

#[test]
fn prompt_needs_a_word_most_notes_do_not_share() {
    // Few notes: no word is generic, so the two shared words match.
    let (home, vault, repo) = memory_home();
    let few: Vec<String> = (20..23).map(deploy_note).collect();
    let few: Vec<&str> = few.iter().map(String::as_str).collect();
    add_notes(home.path(), &vault, &repo, &few);
    let stdin = prompt_json(&repo, "s-1", "deploy the server");
    let out = hook("prompt", &repo, home.path(), &vault, &stdin);
    assert!(stdout(&out).contains("Deploy rule"), "{out:?}");

    // Once most of many notes hold both words, they say nothing about which
    // note a prompt is about.
    let (home, vault, repo) = memory_home();
    let many: Vec<String> = (20..26).map(deploy_note).collect();
    let mut many: Vec<&str> = many.iter().map(String::as_str).collect();
    many.push(RETRY);
    add_notes(home.path(), &vault, &repo, &many);
    let stdin = prompt_json(&repo, "s-1", "deploy the server");
    let out = hook("prompt", &repo, home.path(), &vault, &stdin);
    assert_eq!((code(&out), stdout(&out)), (0, String::new()));
    // A word only one of them holds still picks that one.
    let stdin = prompt_json(&repo, "s-2", "deploy and check item 23");
    let out = hook("prompt", &repo, home.path(), &vault, &stdin);
    assert!(stdout(&out).contains("Deploy rule 23"), "{out:?}");
}

#[test]
fn prompt_skips_a_note_the_session_was_already_shown() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[RETRY]);
    let retry = "retry the failed migration";

    // Session start shows every note to s-1.
    let out = session_start(&repo, home.path(), &vault, &cwd_json(&repo));
    assert!(stdout(&out).contains("Retry the migration"), "{out:?}");
    let out = hook(
        "prompt",
        &repo,
        home.path(),
        &vault,
        &prompt_json(&repo, "s-1", retry),
    );
    assert_eq!(stdout(&out), "");

    // Another session has not seen it, and sees it once.
    let s2 = prompt_json(&repo, "s-2", retry);
    let out = hook("prompt", &repo, home.path(), &vault, &s2);
    assert_eq!(stdout(&out), prompt_reply(RETRY_FOR_PROMPT));
    let out = hook("prompt", &repo, home.path(), &vault, &s2);
    assert_eq!(stdout(&out), "");

    // Without a session id there is nothing to remember it by.
    let anonymous = serde_json::json!({ "cwd": repo, "prompt": retry }).to_string();
    for _ in 0..2 {
        let out = hook("prompt", &repo, home.path(), &vault, &anonymous);
        assert_eq!(stdout(&out), prompt_reply(RETRY_FOR_PROMPT));
    }
}

/// Codex's PreToolUse hook JSON for an `apply_patch` of `patch`.
fn patch_json(cwd: &Path, patch: &str) -> String {
    serde_json::json!({
        "session_id": "s-1",
        "cwd": cwd,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": { "command": patch },
    })
    .to_string()
}

#[test]
fn pre_edit_reads_the_files_a_codex_patch_names() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[RETRY, PIN, GLOBAL]);
    let patch = "*** Begin Patch\n\
                 *** Add File: src/b.rs\n\
                 +fn b() {}\n\
                 *** Update File: src/a.rs\n\
                 @@\n\
                 -fn retry() {}\n\
                 +fn retry_once() {}\n\
                 *** End Patch\n";

    let expected = context_reply(
        "Lore notes anchored to src/a.rs:\n\
         - [fresh] Retry the migration (01JD0000000000000000000001)\n",
    );
    let out = hook(
        "pre-edit",
        &repo,
        home.path(),
        &vault,
        &patch_json(&repo, patch),
    );
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), expected);

    for payload in [
        serde_json::json!({
            "session_id": "s-1",
            "cwd": &repo,
            "hook_event_name": "PreToolUse",
            "tool_name": "apply_patch",
            "tool_input": { "patch": patch },
        }),
        serde_json::json!({
            "session_id": "s-1",
            "cwd": &repo,
            "hook_event_name": "PreToolUse",
            "tool_name": "apply_patch",
            "tool_input": { "input": patch },
        }),
        serde_json::json!({
            "session_id": "s-1",
            "cwd": &repo,
            "hook_event_name": "PreToolUse",
            "tool_name": "apply_patch",
            "tool_input": patch,
        }),
    ] {
        let out = hook("pre-edit", &repo, home.path(), &vault, &payload.to_string());
        assert_eq!(stdout(&out), expected);

        let out = hook("pre-tool", &repo, home.path(), &vault, &payload.to_string());
        assert_eq!(stdout(&out), expected);
    }

    // A move names the file it moves to; a patch with no header names none.
    let moved =
        "*** Begin Patch\n*** Update File: src/old.rs\n*** Move to: src/a.rs\n*** End Patch\n";
    let out = hook(
        "pre-edit",
        &repo,
        home.path(),
        &vault,
        &patch_json(&repo, moved),
    );
    assert!(stdout(&out).contains("Retry the migration"), "{out:?}");
    for patch in ["*** Begin Patch\n*** End Patch\n", "", "src/a.rs"] {
        let out = hook(
            "pre-edit",
            &repo,
            home.path(),
            &vault,
            &patch_json(&repo, patch),
        );
        assert_eq!((code(&out), stdout(&out)), (0, String::new()), "{patch}");
    }
}

// Made-up notes with `enforce` rules. The ids are their own, so a test can
// tell which note a deny names.
const NO_ATTRIBUTION: &str = r"---
id: 01JD0000000000000000000011
type: convention
title: Never add an AI co-author line
enforce:
  - tool: bash
    pattern: '(?i)co-authored-by:\s*(claude|codex)'
    unless: 'LORE-TEST-EXEMPT'
---
Commit messages carry no AI co-author trailer.
";
const NO_TODO_MARKERS: &str = r"---
id: 01JD0000000000000000000012
type: convention
title: Leave no XXX markers in code
enforce:
  - tool: write
    pattern: 'XXX\('
---
File an issue instead of leaving a marker.
";
const NO_PLAN_FILES: &str = r"---
id: 01JD0000000000000000000013
type: convention
title: Never write planning docs into the repo
scope: repo:81444c90f1ff44ac
enforce:
  - tool: path
    pattern: '(^|/)PLAN\.md$'
---
Planning docs live outside the repository.
";

/// Claude Code's PreToolUse hook JSON for `tool` with `input`.
fn tool_json(cwd: &Path, tool: &str, input: serde_json::Value) -> String {
    serde_json::json!({
        "session_id": "s-1",
        "cwd": cwd,
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": input,
    })
    .to_string()
}

/// The reason of a PreToolUse deny in `out`, or `None` when it printed none.
fn deny_reason(out: &Output) -> Option<String> {
    let reply: serde_json::Value = serde_json::from_str(stdout(out).trim()).ok()?;
    let specific = &reply["hookSpecificOutput"];
    assert_eq!(specific["hookEventName"], "PreToolUse");
    (specific["permissionDecision"] == "deny").then(|| {
        specific["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .to_owned()
    })
}

#[test]
fn pre_tool_denies_a_bash_command_a_note_rule_matches() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[NO_ATTRIBUTION]);
    let bash = |command: &str| {
        let stdin = tool_json(&repo, "Bash", serde_json::json!({ "command": command }));
        hook("pre-tool", &repo, home.path(), &vault, &stdin)
    };

    let out = bash("git commit -m 'fix: x' -m 'Co-Authored-By: Claude <noreply@anthropic.com>'");
    assert_eq!(code(&out), 0);
    assert_eq!(
        deny_reason(&out).as_deref(),
        Some(
            "Lore note 01JD0000000000000000000011: Never add an AI co-author line. \
             The command matches `(?i)co-authored-by:\\s*(claude|codex)`: \
             \"Co-Authored-By: Claude\". Follow the note instead."
        )
    );
    // `unless` lets the same command through, and a clean one prints nothing.
    let exempt = bash("git commit -m 'Co-Authored-By: Claude' # LORE-TEST-EXEMPT");
    assert_eq!((code(&exempt), stdout(&exempt)), (0, String::new()));
    let clean = bash("git commit -m 'fix: x'");
    assert_eq!((code(&clean), stdout(&clean)), (0, String::new()));

    // Codex sends its command as an argument list.
    let stdin = tool_json(
        &repo,
        "Bash",
        serde_json::json!({ "command": ["bash", "-lc", "git commit -m 'Co-authored-by: codex'"] }),
    );
    let out = hook("pre-tool", &repo, home.path(), &vault, &stdin);
    assert!(deny_reason(&out)
        .unwrap()
        .contains("01JD0000000000000000000011"));
}

#[test]
fn pre_tool_denies_what_a_patch_or_a_write_adds() {
    let (home, vault, repo) = memory_home();
    add_notes(
        home.path(),
        &vault,
        &repo,
        &[NO_TODO_MARKERS, NO_PLAN_FILES],
    );
    let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-fn retry() {}\n+fn retry() { XXX(later) }\n*** End Patch\n";
    let out = hook(
        "pre-tool",
        &repo,
        home.path(),
        &vault,
        &patch_json(&repo, patch),
    );
    let reason = deny_reason(&out).unwrap();
    assert!(
        reason.starts_with("Lore note 01JD0000000000000000000012: "),
        "{reason}"
    );
    assert!(reason.contains("The text written matches"), "{reason}");

    // A removed line is not written, so it matches nothing.
    let removing = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-XXX(old)\n+fn retry() {}\n*** End Patch\n";
    let out = hook(
        "pre-tool",
        &repo,
        home.path(),
        &vault,
        &patch_json(&repo, removing),
    );
    assert_eq!(deny_reason(&out), None);

    let plan = repo.join("docs/PLAN.md");
    let write = tool_json(
        &repo,
        "Write",
        serde_json::json!({ "file_path": plan, "content": "step one" }),
    );
    let out = hook("pre-tool", &repo, home.path(), &vault, &write);
    let reason = deny_reason(&out).unwrap();
    assert!(
        reason.starts_with("Lore note 01JD0000000000000000000013: "),
        "{reason}"
    );
    assert!(reason.contains("The path written matches"), "{reason}");
    let fine = tool_json(
        &repo,
        "Write",
        serde_json::json!({ "file_path": repo.join("docs/PLANS.txt"), "content": "x" }),
    );
    let out = hook("pre-tool", &repo, home.path(), &vault, &fine);
    assert_eq!((code(&out), stdout(&out)), (0, String::new()));
}

#[test]
fn a_write_rule_matches_only_the_lines_a_call_adds() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[NO_TODO_MARKERS]);
    std::fs::write(repo.join("src/old.rs"), "fn a() {}\n// XXX(kept) fixture\n").unwrap();
    let run = |tool: &str, input: serde_json::Value| {
        let out = hook(
            "pre-tool",
            &repo,
            home.path(),
            &vault,
            &tool_json(&repo, tool, input),
        );
        assert_eq!(code(&out), 0);
        deny_reason(&out)
    };

    // Rewriting a file that already holds the line is allowed, whether the
    // path is absolute or relative to the hook's cwd.
    let rewrite = "fn a() {}\nfn b() {}\n// XXX(kept) fixture\n";
    for path in [repo.join("src/old.rs"), PathBuf::from("src/old.rs")] {
        let input = serde_json::json!({ "file_path": path, "content": rewrite });
        assert_eq!(run("Write", input), None);
    }
    // Adding a new matching line to it is denied.
    let adding = format!("{rewrite}// XXX(new)\n");
    let input = serde_json::json!({ "file_path": "src/old.rs", "content": adding });
    assert!(run("Write", input).is_some());
    // A new file holding the line is denied: every line of it is added.
    let input = serde_json::json!({ "file_path": repo.join("src/new.rs"), "content": rewrite });
    assert!(run("Write", input).is_some());

    // An Edit that keeps a line of its old_string is allowed; one that adds
    // the line is denied. A MultiEdit is read edit by edit.
    let keep = serde_json::json!({ "file_path": "src/old.rs",
        "old_string": "// XXX(kept) fixture", "new_string": "fn c() {}\n// XXX(kept) fixture" });
    assert_eq!(run("Edit", keep.clone()), None);
    let add = serde_json::json!({ "file_path": "src/old.rs",
        "old_string": "fn a() {}", "new_string": "fn a() {}\n// XXX(kept) fixture" });
    assert!(run("Edit", add.clone()).is_some());
    let multi = |edits: serde_json::Value| {
        run(
            "MultiEdit",
            serde_json::json!({ "file_path": "src/old.rs", "edits": edits }),
        )
    };
    assert_eq!(multi(serde_json::json!([keep.clone(), keep.clone()])), None);
    assert!(multi(serde_json::json!([keep, add])).is_some());
}

#[test]
fn pre_tool_shows_anchored_notes_when_no_rule_matches() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[RETRY, NO_TODO_MARKERS]);
    let file = repo.join("src/a.rs");
    let out = hook(
        "pre-tool",
        &repo,
        home.path(),
        &vault,
        &edit_json(&repo, file.to_str().unwrap()),
    );
    assert_eq!(
        stdout(&out),
        context_reply(
            "Lore notes anchored to src/a.rs:\n\
             - [fresh] Retry the migration (01JD0000000000000000000001)\n"
        )
    );
}

#[test]
fn a_rule_blocks_nothing_when_retired_out_of_scope_or_invalid() {
    let (home, vault, repo) = memory_home();
    add_notes(home.path(), &vault, &repo, &[NO_ATTRIBUTION, NO_PLAN_FILES]);
    let attribution = |cwd: &Path| {
        let stdin = tool_json(
            cwd,
            "Bash",
            serde_json::json!({ "command": "git commit -m 'Co-Authored-By: Claude'" }),
        );
        hook("pre-tool", cwd, home.path(), &vault, &stdin)
    };
    assert!(deny_reason(&attribution(&repo)).is_some());

    // A repository's rule does not follow the agent into another repository.
    let other = home.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q", "-b", "main"]);
    git(
        &other,
        &["remote", "add", "origin", "git@github.com:someone/else.git"],
    );
    let plan = |cwd: &Path| {
        let stdin = tool_json(
            cwd,
            "Write",
            serde_json::json!({ "file_path": cwd.join("PLAN.md"), "content": "x" }),
        );
        hook("pre-tool", cwd, home.path(), &vault, &stdin)
    };
    assert!(deny_reason(&plan(&repo)).is_some());
    let out = plan(&other);
    assert_eq!((code(&out), stdout(&out)), (0, String::new()));

    // A retired note's rule no longer applies.
    mem(
        home.path(),
        &vault,
        &["retire", "01JD0000000000000000000011", "--reason", "test"],
    );
    let out = attribution(&repo);
    assert_eq!((code(&out), stdout(&out)), (0, String::new()));

    // A rule whose pattern does not compile is dropped, not the note: the
    // note is added, recalled, and shown at session start, the rule blocks
    // nothing, and every place that lists problems names it.
    let broken = NO_ATTRIBUTION
        .replace("0011", "0019")
        .replace("Never add an AI", "Still never add an AI")
        .replace("(claude|codex)'", "(claude'");
    let file = home.path().join("broken.md");
    std::fs::write(&file, &broken).unwrap();
    let lorectl_mem = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_lorectl"))
            .arg("mem")
            .args(args)
            .arg("--vault")
            .arg(&vault)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", home.path())
            .env_remove("LORE_VAULT_DIR")
            .output()
            .unwrap()
    };
    let added = lorectl_mem(&["add", file.to_str().unwrap()]);
    assert_eq!(
        code(&added),
        0,
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let stderr = String::from_utf8_lossy(&added.stderr);
    assert!(
        stderr.contains(
            "rule `enforce[0].pattern`: `(?i)co-authored-by:\\s*(claude` is not a valid regex"
        ),
        "{stderr}"
    );
    let rel = stdout(&added)
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    let out = attribution(&repo);
    assert_eq!((code(&out), stdout(&out)), (0, String::new()));
    let recalled = lorectl_mem(&["recall", "co-author"]);
    assert!(
        stdout(&recalled).contains("still-never-add-an-ai-co-author-line"),
        "{}",
        stdout(&recalled)
    );
    let started = session_start(&repo, home.path(), &vault, "{}");
    assert!(
        stdout(&started).contains("Still never add an AI co-author line"),
        "{}",
        stdout(&started)
    );
    let reindexed = lorectl_mem(&["reindex"]);
    assert_eq!(code(&reindexed), 0);
    assert!(stdout(&reindexed).contains("skipped 0 files"));
    let stderr = String::from_utf8_lossy(&reindexed.stderr);
    assert!(
        stderr.contains(&format!("lorectl: {rel}: rule `enforce[0].pattern`: "))
            && stderr.contains("the rule blocks nothing"),
        "{stderr}"
    );
    let check = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("--archive")
        .arg(home.path().join("no-archive"))
        .args(["mem", "enforce-check", "--vault"])
        .arg(&vault)
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(code(&check), 7);
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains(&format!(
            "lorectl: invalid rule in 01JD0000000000000000000019 \
             Still never add an AI co-author line ({rel}): `enforce[0].pattern`"
        )),
        "{stderr}"
    );
    let listed = stdout(&check);
    assert!(listed.contains("no archive to replay against"), "{listed}");
    assert!(
        listed.contains("01JD0000000000000000000013  Never write planning docs"),
        "{listed}"
    );
    assert!(
        !listed.contains("01JD0000000000000000000011"),
        "retired: {listed}"
    );
}

/// [`hook`], killed after ten seconds, with how long it took.
fn timed_hook(
    event: &str,
    cwd: &Path,
    home: &Path,
    vault: &Path,
    stdin: &str,
) -> (Output, std::time::Duration) {
    let started = Instant::now();
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
    while child.try_wait().unwrap().is_none() {
        if started.elapsed().as_secs() >= 10 {
            child.kill().unwrap();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let elapsed = started.elapsed();
    (child.wait_with_output().unwrap(), elapsed)
}

/// A folder that looks like a repository but whose `.git/HEAD` is a FIFO, so
/// opening it blocks, as an open under a folder macOS is asking the person
/// about does until they answer.
fn stuck_folder(home: &Path) -> PathBuf {
    let stuck = home.join("stuck");
    std::fs::create_dir_all(stuck.join(".git/objects")).unwrap();
    std::fs::create_dir_all(stuck.join(".git/refs")).unwrap();
    let fifo = Command::new("mkfifo")
        .arg(stuck.join(".git/HEAD"))
        .status()
        .unwrap();
    assert!(fifo.success(), "mkfifo failed");
    stuck
}

/// A parent that writes the event and keeps the pipe open (or a child that
/// inherited its write end) must not hold the hook until the pipe closes.
#[test]
fn a_hook_answers_while_its_stdin_stays_open() {
    let (home, vault, repo) = memory_home();
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["hook", "prompt", "--archive"])
        .arg(home.path().join("archive"))
        .current_dir(&repo)
        .env("HOME", home.path())
        .env("LORE_VAULT_DIR", &vault)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("LORE_ARCHIVE_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lorectl");
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(prompt_json(&repo, "s-open", "hello").as_bytes())
        .unwrap();
    pipe.flush().unwrap();
    let exited = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        if started.elapsed().as_secs() >= 5 {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    drop(pipe);
    let out = child.wait_with_output().unwrap();
    assert!(exited, "the hook waited for stdin to close");
    assert_eq!(code(&out), 0);
}

#[test]
fn hooks_go_on_without_a_working_tree_that_does_not_answer() {
    let (home, vault, _repo) = memory_home();
    for (i, text) in [NO_TODO_MARKERS, NO_PLAN_FILES].iter().enumerate() {
        let file = home.path().join(format!("rule-{i}.md"));
        std::fs::write(&file, text).unwrap();
        mem(home.path(), &vault, &["add", file.to_str().unwrap()]);
    }
    let stuck = stuck_folder(home.path());
    let runs = [
        ("session-start", cwd_json(&stuck)),
        (
            "prompt",
            prompt_json(&stuck, "s-1", "leave markers in planning docs"),
        ),
        (
            "pre-tool",
            tool_json(
                &stuck,
                "Write",
                serde_json::json!({ "file_path": stuck.join("a.rs"), "content": "XXX(later)" }),
            ),
        ),
    ];
    for (event, stdin) in runs {
        let (out, elapsed) = timed_hook(event, &stuck, home.path(), &vault, &stdin);
        assert!(elapsed.as_secs() < 2, "{event} took {elapsed:?}");
        assert_eq!(code(&out), 0, "{event}");
        if event == "pre-tool" {
            // Global rules still hold where the repository cannot be read.
            let reason = deny_reason(&out).unwrap();
            assert!(reason.contains("01JD0000000000000000000012"), "{reason}");
        } else {
            assert_eq!(stdout(&out), "", "{event}");
        }
    }
}
