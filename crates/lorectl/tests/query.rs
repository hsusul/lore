//! `lorectl search` and `lorectl inspect` end to end, as real processes.
//!
//! These are the first commands that can meet an archive which does not exist —
//! `scan` creates one — so this is where exit code 2 finally gets exercised.
//!
//! Agent homes are isolated per child process; no real `~/.claude` or `~/.codex`
//! history is read (`docs/development/TESTING.md`).
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::process::{Command, Output};

fn lorectl(archive: &Path, homes: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .arg("--archive")
        .arg(archive)
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

/// A scanned archive holding a synthetic profile, plus the homes guard.
fn scanned_archive() -> (tempfile::TempDir, tempfile::TempDir) {
    let archive = tempfile::tempdir().unwrap();
    let homes = tempfile::tempdir().unwrap();
    lore_core::synthetic::generate(
        homes.path(),
        &lore_core::synthetic::ProfileSpec {
            claude_sessions: 3,
            codex_sessions: 2,
            max_extra_turns: 2,
            seed: 11,
        },
    )
    .unwrap();
    let out = lorectl(archive.path(), homes.path(), &["scan"]);
    assert_eq!(code(&out), 0, "seeding scan failed");
    (archive, homes)
}

fn a_session_id(archive: &Path) -> String {
    let conn = lore_core::storage::open_read_only(&archive.join("lore.db")).unwrap();
    conn.query_row(
        "SELECT id FROM agent_session ORDER BY id LIMIT 1",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn searching_a_missing_archive_exits_no_archive() {
    // Exit 2 was unreachable until a read command existed: `scan` creates the
    // archive it is pointed at, so it can never meet a missing one.
    let parent = tempfile::tempdir().unwrap();
    let homes = tempfile::tempdir().unwrap();
    let missing = parent.path().join("not-an-archive");

    let out = lorectl(&missing, homes.path(), &["search", "anything"]);
    assert_eq!(code(&out), 2, "a missing archive must exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no archive"), "{stderr}");
    assert!(
        stderr.contains("lorectl scan"),
        "must name the fix: {stderr}"
    );
    assert!(!missing.exists(), "a read command created an archive");
}

#[test]
fn inspecting_a_missing_archive_exits_no_archive() {
    let parent = tempfile::tempdir().unwrap();
    let homes = tempfile::tempdir().unwrap();
    let missing = parent.path().join("not-an-archive");

    let out = lorectl(&missing, homes.path(), &["inspect", "whatever"]);
    assert_eq!(code(&out), 2);
    assert!(!missing.exists());
}

#[test]
fn a_search_with_no_matches_succeeds_and_says_so() {
    // "I looked and found nothing" is a successful search. Exiting non-zero
    // would make an empty result indistinguishable from a broken one.
    let (archive, homes) = scanned_archive();
    let out = lorectl(
        archive.path(),
        homes.path(),
        &["search", "zzz-no-such-term-zzz"],
    );
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("no matches"), "{}", stdout(&out));
}

#[test]
fn a_search_finds_archived_content() {
    let (archive, homes) = scanned_archive();
    let out = lorectl(archive.path(), homes.path(), &["--json", "search", "the"]);
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Each line is one hit, so a caller can stream them.
    // Asserted before the loop: the loop is vacuous on zero hits, so without
    // this the test passed whether or not search found anything.
    let lines: Vec<String> = stdout(&out).lines().map(str::to_string).collect();
    assert!(
        !lines.is_empty(),
        "search found nothing in a freshly scanned synthetic profile"
    );
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line).expect("each line is JSON");
        assert!(value["session_id"].as_str().is_some());
        assert!(value["snippet"].as_str().is_some());
    }
}

#[test]
fn inspect_reports_a_session_that_exists() {
    let (archive, homes) = scanned_archive();
    let sid = a_session_id(archive.path());

    let out = lorectl(archive.path(), homes.path(), &["--json", "inspect", &sid]);
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(value["session_id"], sid);
    assert!(value["message_count"].as_i64().unwrap() > 0);
}

#[test]
fn inspecting_an_unknown_session_is_a_usage_error_not_an_archive_error() {
    // The archive is fine; the argument was wrong. Reporting this as an archive
    // problem would send someone to debug their database.
    let (archive, homes) = scanned_archive();
    let out = lorectl(
        archive.path(),
        homes.path(),
        &["inspect", "no-such-session"],
    );
    assert_eq!(code(&out), 1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no session"), "{stderr}");
}

#[test]
fn a_missing_operand_is_refused_before_the_archive_is_opened() {
    // Pointed at an archive that does not exist: if this exited 2, the operand
    // check would be happening too late.
    let parent = tempfile::tempdir().unwrap();
    let homes = tempfile::tempdir().unwrap();
    let missing = parent.path().join("nope");

    for args in [vec!["search"], vec!["inspect"]] {
        let out = lorectl(&missing, homes.path(), &args);
        assert_eq!(code(&out), 1, "{args:?} should be a usage error");
    }
}

#[test]
fn reading_an_archive_does_not_modify_it() {
    // The promise a query surface owes an archive: same directory before and
    // after, including no `blobs/tmp/` conjured by a writer-style blob open.
    let (archive, homes) = scanned_archive();
    let sid = a_session_id(archive.path());

    let before = common::listing(archive.path());
    assert_eq!(
        code(&lorectl(archive.path(), homes.path(), &["search", "the"])),
        0
    );
    assert_eq!(
        code(&lorectl(
            archive.path(),
            homes.path(),
            &["inspect", "--patch", &sid]
        )),
        0
    );
    assert_eq!(
        before,
        common::listing(archive.path()),
        "a read changed the archive"
    );
}

#[test]
fn inspect_patch_runs_against_a_session_with_no_recorded_patches() {
    // The synthetic profile records no patches, so `--patch` must simply add
    // nothing rather than fail or invent a placeholder session.
    let (archive, homes) = scanned_archive();
    let sid = a_session_id(archive.path());

    let plain = lorectl(archive.path(), homes.path(), &["inspect", &sid]);
    let patched = lorectl(archive.path(), homes.path(), &["inspect", "--patch", &sid]);
    assert_eq!(code(&plain), 0);
    assert_eq!(code(&patched), 0);
    assert!(
        stdout(&patched).starts_with(stdout(&plain).trim_end()),
        "--patch changed the session header"
    );
}

#[test]
fn a_read_command_does_not_take_the_writer_lock() {
    // A query must not be blocked by a running scan; that is the entire reason
    // reads open read-only instead of taking the lock.
    let (archive, homes) = scanned_archive();
    let _held = lore_core::lock::ScanLock::acquire(archive.path()).expect("hold the writer lock");

    let out = lorectl(archive.path(), homes.path(), &["search", "the"]);
    assert_eq!(
        code(&out),
        0,
        "a read was refused while the writer lock was held: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
