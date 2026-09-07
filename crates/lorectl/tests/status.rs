//! `lorectl status` end to end.
//!
//! `status` is the command the product thesis rests on, so most of these tests
//! are about what it is **not** allowed to say. Lore cannot yet prove anything
//! about whether archived work landed in Git, and the failure mode that matters
//! is not a crash — it is a confident sentence that outruns the evidence.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::process::{Command, Output};

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

fn code(output: &Output) -> i32 {
    output.status.code().expect("exited with a code")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("git must be installed");
    assert!(out.status.success(), "git {args:?} failed");
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial"]);
    dir
}

fn empty_homes() -> tempfile::TempDir {
    let homes = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(homes.path().join("claude")).unwrap();
    std::fs::create_dir_all(homes.path().join("codex")).unwrap();
    homes
}

/// Words that assert something about whether work reached a commit. None of
/// them is provable by this build, so none may appear in its output.
const FORBIDDEN: &[&str] = &[
    "lost",
    "abandoned",
    "uncommitted",
    "unfinished",
    "never landed",
    "not landed",
    "landed",
];

#[test]
fn status_never_claims_anything_about_landing() {
    // The epistemic constraint, enforced rather than trusted. If a later change
    // teaches Lore to prove landing, this test should be updated deliberately —
    // which is the point of failing here.
    let repo = repo();
    let homes = empty_homes();
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

    let out = lorectl_in(repo.path(), archive.path(), homes.path(), &["status"]);
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = stdout(&out).to_lowercase();
    for word in FORBIDDEN {
        // "not assessed by this build" is the one sanctioned mention, and it
        // denies a claim rather than making one.
        let stripped = text.replace(
            "whether this work landed in git is not assessed by this build",
            "",
        );
        assert!(
            !stripped.contains(word),
            "status claimed `{word}`, which this build cannot prove:\n{}",
            stdout(&out)
        );
    }
}

#[test]
fn status_always_says_when_lore_last_looked() {
    // A session count without this is uninterpretable: "none recorded" and "no
    // scan has ever run" would look identical to a reader.
    let repo = repo();
    let homes = empty_homes();
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

    let out = lorectl_in(repo.path(), archive.path(), homes.path(), &["status"]);
    assert!(stdout(&out).contains("last scan"), "{}", stdout(&out));
}

#[test]
fn a_never_scanned_archive_says_never_rather_than_implying_emptiness() {
    // The case most likely to be misread as "no agent ever worked here".
    let repo = repo();
    let homes = empty_homes();
    let archive = tempfile::tempdir().unwrap();
    // An archive that exists but has never completed a scan.
    let conn = lore_core::storage::open(&archive.path().join("lore.db")).unwrap();
    drop(conn);

    let out = lorectl_in(repo.path(), archive.path(), homes.path(), &["status"]);
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = stdout(&out);
    assert!(text.contains("never"), "{text}");
    assert!(text.contains("lorectl scan"), "must name the fix: {text}");
}

#[test]
fn status_outside_a_repository_exits_not_a_repo() {
    // Exit 3, reserved by the first CLI slice and claimed here. It must not be
    // an archive code: the archive is fine and simply not the problem.
    let plain = tempfile::tempdir().unwrap();
    let homes = empty_homes();
    let archive = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&lorectl_in(
            plain.path(),
            archive.path(),
            homes.path(),
            &["scan"]
        )),
        0
    );

    let out = lorectl_in(plain.path(), archive.path(), homes.path(), &["status"]);
    assert_eq!(
        code(&out),
        3,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Git repository"), "{stderr}");
}

#[test]
fn status_on_a_missing_archive_exits_no_archive() {
    let repo = repo();
    let homes = empty_homes();
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("nope");

    let out = lorectl_in(repo.path(), &missing, homes.path(), &["status"]);
    assert_eq!(code(&out), 2);
    assert!(!missing.exists(), "status created an archive");
}

#[test]
fn a_repository_with_no_recorded_sessions_says_so_as_a_fact_about_the_archive() {
    // Phrasing matters: this is what the archive holds, not what happened.
    let repo = repo();
    let homes = empty_homes();
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

    let out = lorectl_in(repo.path(), archive.path(), homes.path(), &["status"]);
    let text = stdout(&out);
    assert!(
        text.contains("none recorded in this archive"),
        "must attribute the absence to the archive: {text}"
    );
}

#[test]
fn status_json_reports_landing_as_counts_per_rung() {
    // Counts, never a score, and never a bare boolean: "committed" and "not
    // assessed" answer different questions, and a reader must be able to see
    // how much of the work Lore had no evidence for.
    let repo = repo();
    let homes = empty_homes();
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
        &["--json", "status"],
    );
    assert_eq!(code(&out), 0);
    let value: serde_json::Value = serde_json::from_str(stdout(&out).trim()).expect("valid JSON");
    let landing = &value["landing"];
    for rung in [
        "committed",
        "in_repository_not_on_a_branch",
        "no_observed_landing",
        "not_assessed",
    ] {
        assert!(
            landing[rung].as_u64().is_some(),
            "rung `{rung}` missing from {landing}"
        );
    }
    let assesses: Vec<&str> = value["assesses"]
        .as_array()
        .expect("assesses lists what this output covers")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(assesses.contains(&"landing"));
    assert!(assesses.contains(&"last_scan"));
}

#[test]
fn status_json_distinguishes_never_scanned_from_scanned_and_empty() {
    // The text path says "never"; JSON has to make the same distinction, or a
    // machine reader cannot tell "Lore has not looked" from "Lore looked and
    // found nothing".
    let repo = repo();
    let homes = empty_homes();

    let never = tempfile::tempdir().unwrap();
    drop(lore_core::storage::open(&never.path().join("lore.db")).unwrap());
    let out = lorectl_in(
        repo.path(),
        never.path(),
        homes.path(),
        &["--json", "status"],
    );
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert!(
        value["last_scan_completed_at_ms"].is_null(),
        "a never-scanned archive must report null, not a time: {value}"
    );

    let scanned = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&lorectl_in(
            repo.path(),
            scanned.path(),
            homes.path(),
            &["scan"]
        )),
        0
    );
    let out = lorectl_in(
        repo.path(),
        scanned.path(),
        homes.path(),
        &["--json", "status"],
    );
    let value: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert!(
        value["last_scan_completed_at_ms"].as_i64().unwrap() > 1_700_000_000_000,
        "a scanned archive must report when: {value}"
    );
}

#[test]
fn status_reports_sessions_recorded_for_this_repository() {
    // The positive case: a session whose cwd is this repository is counted.
    let repo = repo();
    let homes = empty_homes();
    let archive = tempfile::tempdir().unwrap();

    // A Claude session recorded as having run in this repository.
    let project = homes.path().join("claude/projects/p");
    std::fs::create_dir_all(&project).unwrap();
    let line = format!(
        "{{\"type\":\"user\",\"uuid\":\"u1\",\"sessionId\":\"s-status\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n",
        repo.path().display()
    );
    std::fs::write(
        project.join("11111111-2222-4333-8444-555555555555.jsonl"),
        line,
    )
    .unwrap();

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
        &["--json", "status"],
    );
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let value: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert!(value["in_archive"].as_bool().unwrap(), "{value}");
    assert_eq!(value["sessions"], 1, "{value}");
    assert_eq!(value["recent"].as_array().unwrap().len(), 1);
}

#[test]
fn status_answers_the_same_from_a_subdirectory() {
    // Standing deeper in the tree is the same repository, so the answer must
    // not change.
    let repo = repo();
    let homes = empty_homes();
    let archive = tempfile::tempdir().unwrap();
    let nested = repo.path().join("src/deep");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(
        code(&lorectl_in(
            repo.path(),
            archive.path(),
            homes.path(),
            &["scan"]
        )),
        0
    );

    let top = lorectl_in(
        repo.path(),
        archive.path(),
        homes.path(),
        &["--json", "status"],
    );
    let deep = lorectl_in(&nested, archive.path(), homes.path(), &["--json", "status"]);
    let a: serde_json::Value = serde_json::from_str(stdout(&top).trim()).unwrap();
    let b: serde_json::Value = serde_json::from_str(stdout(&deep).trim()).unwrap();
    assert_eq!(a["repository_id"], b["repository_id"]);
    assert_eq!(a["sessions"], b["sessions"]);
}

#[test]
fn status_does_not_modify_the_archive() {
    let repo = repo();
    let homes = empty_homes();
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

    let before = common::listing(archive.path());
    assert_eq!(
        code(&lorectl_in(
            repo.path(),
            archive.path(),
            homes.path(),
            &["status"]
        )),
        0
    );
    assert_eq!(
        before,
        common::listing(archive.path()),
        "status changed the archive"
    );
}

#[test]
fn status_works_while_the_writer_lock_is_held() {
    // status is a reader; a running scan must not block it.
    let repo = repo();
    let homes = empty_homes();
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

    let _held = lore_core::lock::ScanLock::acquire(archive.path()).expect("hold the lock");
    let out = lorectl_in(repo.path(), archive.path(), homes.path(), &["status"]);
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
