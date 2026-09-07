//! `lorectl hook` as an agent would actually invoke it.
//!
//! The contract is unusual and worth testing as a contract: a hook runs inside
//! someone's coding session, so it must never fail it. Every test here is really
//! asking the same question — does this stay out of the way?
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::process::{Command, Output};
use std::time::Instant;

fn lorectl_in(cwd: &Path, archive: &Path, homes: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .arg("--archive")
        .arg(archive)
        .current_dir(cwd)
        .env("CLAUDE_CONFIG_DIR", homes.join("claude"))
        .env("CODEX_HOME", homes.join("codex"))
        .env_remove("LORE_ARCHIVE_DIR")
        .output()
        .expect("spawn lorectl")
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
