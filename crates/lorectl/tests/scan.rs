//! `lorectl scan` end to end, as real processes.
//!
//! The scan's contract is about *processes*: it writes to a shared archive, it
//! takes an OS advisory lock, and it reports its outcome through an exit code.
//! None of that is observable from inside a unit test, so these drive the built
//! binary with `CARGO_BIN_EXE_lorectl`.
//!
//! Spawning also sidesteps a real hazard: the adapters read `CLAUDE_CONFIG_DIR`
//! and `CODEX_HOME` from the environment, and mutating those in-process is racy
//! under the parallel test harness (`lore_core::paths` exists partly to avoid
//! exactly that). A child process gets its own environment, so these tests can
//! isolate the agent homes without touching the parent's.
//!
//! Every test here points the adapters at empty temporary directories: no real
//! `~/.claude` or `~/.codex` history is read (`docs/development/TESTING.md`).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

/// Run `lorectl` with isolated agent homes and an explicit archive.
fn lorectl(archive: &Path, homes: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .arg("--archive")
        .arg(archive)
        .env("CLAUDE_CONFIG_DIR", homes.join("claude"))
        .env("CODEX_HOME", homes.join("codex"))
        // The override must not leak in from the developer's own shell and
        // silently retarget the archive under test.
        .env_remove("LORE_ARCHIVE_DIR")
        .output()
        .expect("spawn lorectl")
}

fn empty_homes() -> tempfile::TempDir {
    let homes = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(homes.path().join("claude")).unwrap();
    std::fs::create_dir_all(homes.path().join("codex")).unwrap();
    homes
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exited with a code")
}

#[test]
fn a_scan_creates_the_archive_it_was_pointed_at() {
    // `scan` is the command exit code 2 tells people to run, so it has to be
    // the thing that brings an archive into existence.
    let archive = tempfile::tempdir().unwrap();
    let homes = empty_homes();
    let target = archive.path().join("fresh");

    let out = lorectl(&target, homes.path(), &["scan"]);
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(target.join("lore.db").is_file(), "no database was created");
    assert!(target.join("blobs").is_dir(), "no blob store was created");
    assert!(
        target.join("scan.lock").is_file(),
        "no lock file was created"
    );
}

#[test]
fn a_scan_of_empty_agent_homes_reports_zero_and_succeeds() {
    // Nothing to ingest is a successful scan, not an error: "I looked and found
    // nothing" and "I could not look" must not share an exit code.
    let archive = tempfile::tempdir().unwrap();
    let homes = empty_homes();

    let out = lorectl(archive.path(), homes.path(), &["scan"]);
    assert_eq!(code(&out), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ingested 0"), "{stdout}");
    assert!(
        stdout.contains(&archive.path().display().to_string()),
        "the archive being written must be visible: {stdout}"
    );
}

#[test]
fn a_second_scan_is_refused_while_the_first_holds_the_lock() {
    // The reason the lock exists: `jobs::recover_running` returns every running
    // job to pending with no owner, so a second writer would reclaim the
    // first's in-flight work.
    let archive = tempfile::tempdir().unwrap();
    let homes = empty_homes();
    // Create the archive first so the contended run fails on the lock only.
    assert_eq!(code(&lorectl(archive.path(), homes.path(), &["scan"])), 0);

    let _held = lore_core::lock::ScanLock::acquire(archive.path()).expect("take the lock");
    let out = lorectl(archive.path(), homes.path(), &["scan"]);

    assert_eq!(code(&out), 5, "a busy archive must exit 5");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("another Lore process"), "{stderr}");
    assert!(
        stderr.contains("again"),
        "must say retrying is the fix: {stderr}"
    );
}

#[test]
fn the_lock_is_released_when_the_scanning_process_exits() {
    // A crashed or finished scan must not leave the archive permanently busy.
    let archive = tempfile::tempdir().unwrap();
    let homes = empty_homes();
    assert_eq!(code(&lorectl(archive.path(), homes.path(), &["scan"])), 0);
    assert_eq!(
        code(&lorectl(archive.path(), homes.path(), &["scan"])),
        0,
        "the previous scan's lock outlived it"
    );
}

#[test]
fn a_scan_stamps_when_it_last_looked() {
    // Week-2 status has to be able to say when Lore last looked; "no observed
    // landing" is meaningless without it.
    let archive = tempfile::tempdir().unwrap();
    let homes = empty_homes();
    assert_eq!(code(&lorectl(archive.path(), homes.path(), &["scan"])), 0);

    let conn = lore_core::storage::open_read_only(&archive.path().join("lore.db")).unwrap();
    let raw = lore_core::settings::get(&conn, "scan.last_completed_at")
        .unwrap()
        .expect("the completion stamp is persisted");
    let stamp: i64 = raw.parse().expect("stored as a JSON number");
    assert!(stamp > 1_700_000_000_000, "implausible stamp: {stamp}");
}

#[test]
fn a_refused_scan_does_not_stamp_a_completion() {
    // The stamp answers "when did Lore last look?". A scan that never ran must
    // not move it, or the answer becomes a claim Lore cannot support.
    let archive = tempfile::tempdir().unwrap();
    let homes = empty_homes();
    assert_eq!(code(&lorectl(archive.path(), homes.path(), &["scan"])), 0);

    let db = archive.path().join("lore.db");
    let before = {
        let conn = lore_core::storage::open_read_only(&db).unwrap();
        lore_core::settings::get(&conn, "scan.last_completed_at").unwrap()
    };

    let _held = lore_core::lock::ScanLock::acquire(archive.path()).expect("take the lock");
    assert_eq!(code(&lorectl(archive.path(), homes.path(), &["scan"])), 5);

    let after = {
        let conn = lore_core::storage::open_read_only(&db).unwrap();
        lore_core::settings::get(&conn, "scan.last_completed_at").unwrap()
    };
    assert_eq!(before, after, "a refused scan moved the completion stamp");
}

#[test]
fn json_output_is_one_parseable_line() {
    let archive = tempfile::tempdir().unwrap();
    let homes = empty_homes();

    let out = lorectl(archive.path(), homes.path(), &["--json", "scan"]);
    assert_eq!(code(&out), 0);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "{stdout}");

    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    assert_eq!(value["ingested"], 0);
    assert_eq!(value["archive"], archive.path().display().to_string());
    assert!(value["completed_at_ms"].as_i64().unwrap() > 1_700_000_000_000);
}

#[test]
fn a_relative_archive_is_refused_before_anything_is_created() {
    // Exit 1, and no stray directory: a rejected invocation must not leave
    // half an archive behind.
    let cwd = tempfile::tempdir().unwrap();
    let homes = empty_homes();
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["scan", "--archive", "relative/archive"])
        .env("CLAUDE_CONFIG_DIR", homes.path().join("claude"))
        .env("CODEX_HOME", homes.path().join("codex"))
        .env_remove("LORE_ARCHIVE_DIR")
        .current_dir(cwd.path())
        .output()
        .expect("spawn lorectl");

    assert_eq!(code(&out), 1);
    assert!(
        !cwd.path().join("relative").exists(),
        "created an archive anyway"
    );
}

#[test]
fn scan_is_idempotent_over_an_unchanged_archive() {
    // Re-scanning must not re-ingest what has not changed; that is the property
    // a hook calling `scan` on every session start depends on.
    let archive = tempfile::tempdir().unwrap();
    let homes = empty_homes();
    for _ in 0..3 {
        let out = lorectl(archive.path(), homes.path(), &["--json", "scan"]);
        assert_eq!(code(&out), 0);
        let value: serde_json::Value =
            serde_json::from_str(String::from_utf8(out.stdout).unwrap().trim()).unwrap();
        assert_eq!(value["ingested"], 0);
        assert_eq!(value["failed"], 0);
    }
}

/// A synthetic agent home laid out exactly as the adapters expect:
/// `<homes>/claude/projects` and `<homes>/codex/sessions`, which is what
/// `CLAUDE_CONFIG_DIR` / `CODEX_HOME` are pointed at above.
fn seeded_homes(claude: usize, codex: usize) -> (tempfile::TempDir, usize) {
    let homes = tempfile::tempdir().unwrap();
    let profile = lore_core::synthetic::generate(
        homes.path(),
        &lore_core::synthetic::ProfileSpec {
            claude_sessions: claude,
            codex_sessions: codex,
            max_extra_turns: 2,
            seed: 7,
        },
    )
    .unwrap();
    let total = profile.claude_files + profile.codex_files;
    (homes, total)
}

#[test]
fn a_scan_actually_ingests_the_sessions_it_finds() {
    // The tests above prove a scan of nothing succeeds. This one proves a scan
    // of something does the work — otherwise "exit 0, ingested 0" would pass
    // every test here while the command did nothing at all.
    let archive = tempfile::tempdir().unwrap();
    let (homes, total) = seeded_homes(4, 3);

    let out = lorectl(archive.path(), homes.path(), &["--json", "scan"]);
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let value: serde_json::Value =
        serde_json::from_str(String::from_utf8(out.stdout).unwrap().trim()).unwrap();
    assert_eq!(
        value["ingested"], total,
        "every synthetic session should ingest"
    );
    assert_eq!(value["failed"], 0);

    // And the rows are really in the archive, not just in the report.
    let conn = lore_core::storage::open_read_only(&archive.path().join("lore.db")).unwrap();
    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM agent_session", [], |r| r.get(0))
        .unwrap();
    assert_eq!(usize::try_from(sessions).unwrap(), total);
}

#[test]
fn re_scanning_unchanged_sources_does_no_work_and_adds_no_rows() {
    // What makes `scan` cheap enough to run from a session-start hook.
    //
    // Note the mechanism: an unchanged source coalesces at *scheduling*
    // (`SourceSchedule::CoalescedDone`), so it never becomes a job and never
    // reaches the drain. That is why `skipped` stays 0 here rather than counting
    // the untouched sources — the work is avoided a step earlier than that.
    let archive = tempfile::tempdir().unwrap();
    let (homes, total) = seeded_homes(3, 2);

    assert_eq!(code(&lorectl(archive.path(), homes.path(), &["scan"])), 0);
    let out = lorectl(archive.path(), homes.path(), &["--json", "scan"]);
    assert_eq!(code(&out), 0);

    let value: serde_json::Value =
        serde_json::from_str(String::from_utf8(out.stdout).unwrap().trim()).unwrap();
    assert_eq!(value["ingested"], 0, "unchanged sources must not re-ingest");
    assert_eq!(value["failed"], 0);

    let conn = lore_core::storage::open_read_only(&archive.path().join("lore.db")).unwrap();
    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM agent_session", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        usize::try_from(sessions).unwrap(),
        total,
        "a second scan duplicated rows"
    );
}

#[test]
fn a_new_session_appearing_later_is_picked_up_by_the_next_scan() {
    // The CLI has no watcher by design, so "run it again" has to be enough.
    let archive = tempfile::tempdir().unwrap();
    let (homes, first_total) = seeded_homes(2, 1);
    assert_eq!(code(&lorectl(archive.path(), homes.path(), &["scan"])), 0);

    // A genuinely new session: generate a larger profile elsewhere and take a
    // file the first pass never saw. Copying an already-ingested file would
    // carry the same session id and dedupe rather than add.
    let scratch = tempfile::tempdir().unwrap();
    lore_core::synthetic::generate(
        scratch.path(),
        &lore_core::synthetic::ProfileSpec {
            claude_sessions: 6,
            codex_sessions: 0,
            max_extra_turns: 2,
            seed: 7,
        },
    )
    .unwrap();

    let existing = jsonl_names(&homes.path().join("claude/projects"));
    let fresh = all_jsonl(&scratch.path().join("claude/projects"))
        .into_iter()
        .find(|p| !existing.contains(&p.file_name().unwrap().to_owned()))
        .expect("a session the first scan never saw");

    let landing = homes.path().join("claude/projects/late-project");
    std::fs::create_dir_all(&landing).unwrap();
    std::fs::copy(&fresh, landing.join(fresh.file_name().unwrap())).unwrap();

    let out = lorectl(archive.path(), homes.path(), &["--json", "scan"]);
    assert_eq!(code(&out), 0);
    let value: serde_json::Value =
        serde_json::from_str(String::from_utf8(out.stdout).unwrap().trim()).unwrap();
    assert_eq!(value["ingested"], 1, "the new session was not picked up");

    let conn = lore_core::storage::open_read_only(&archive.path().join("lore.db")).unwrap();
    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM agent_session", [], |r| r.get(0))
        .unwrap();
    assert_eq!(usize::try_from(sessions).unwrap(), first_total + 1);
}

/// Every `.jsonl` under `root`, recursively.
fn all_jsonl(root: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    fn walk(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                found.push(path);
            }
        }
    }
    walk(root, &mut found);
    found.sort();
    found
}

/// The file names (not paths) of every session under `root`.
fn jsonl_names(root: &Path) -> std::collections::HashSet<std::ffi::OsString> {
    all_jsonl(root)
        .into_iter()
        .filter_map(|p| p.file_name().map(std::ffi::OsStr::to_owned))
        .collect()
}
