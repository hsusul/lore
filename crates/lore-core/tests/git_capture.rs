//! M4 acceptance: read-only gix capture reports branch/HEAD/dirty state, treats
//! a detached HEAD as branch-less, groups linked worktrees by common dir, and
//! declines a non-git path — over throwaway fixture repositories.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

use lore_core::git::{capture, capture_identity, capture_via_git};

/// Run a git command in `dir` with a hermetic environment (no user/global/system
/// config, no prompts) and a fixed identity so commits succeed deterministically.
fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git must be installed to run M4 capture tests");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Create a repository on branch `main` with one commit and return its path.
fn init_repo(dir: &Path) {
    git(dir, &["init", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "initial"]);
}

#[test]
fn capture_reads_branch_commit_and_clean_state() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());

    let facts = capture(dir.path()).expect("path is inside a repo");
    assert_eq!(facts.branch.as_deref(), Some("main"));
    assert!(!facts.detached);
    assert_eq!(
        facts.head_commit.as_deref().map(str::len),
        Some(40),
        "a born HEAD yields a full commit hash"
    );
    assert_eq!(facts.is_dirty, Some(false), "a fresh commit is clean");
    assert!(facts.workdir.is_some());
}

#[test]
fn capture_detects_a_dirty_worktree() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    std::fs::write(dir.path().join("README.md"), "changed\n").unwrap();

    let facts = capture(dir.path()).unwrap();
    assert_eq!(facts.is_dirty, Some(true));
    assert_eq!(facts.changed_files, Some(vec!["README.md".to_string()]));

    git(dir.path(), &["add", "README.md"]);
    let facts_staged = capture(dir.path()).unwrap();
    assert_eq!(facts_staged.is_dirty, Some(true));
}

#[test]
fn capture_reports_detached_head_with_null_branch() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    git(dir.path(), &["checkout", "--detach", "HEAD"]);

    let facts = capture(dir.path()).unwrap();
    assert!(facts.detached, "HEAD is detached");
    assert_eq!(
        facts.branch, None,
        "detached HEAD stores commit with branch NULL"
    );
    assert_eq!(facts.head_commit.as_deref().map(str::len), Some(40));
}

#[test]
fn capture_reports_normalized_remote_and_root_commit() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://user:tok@github.com/org/repo.git",
        ],
    );

    let facts = capture(dir.path()).unwrap();
    assert_eq!(
        facts.remotes,
        vec!["github.com/org/repo".to_string()],
        "remote is normalized and credential-free"
    );
    assert_eq!(facts.root_commits.len(), 1, "one initial (root) commit");
    assert_eq!(facts.root_commits[0].len(), 40);
    assert!(!facts.history_truncated);
}

#[test]
fn capture_returns_none_outside_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        capture(dir.path()).is_none(),
        "a non-git path is kept under 'No repository'"
    );
}

#[test]
fn hardened_fallback_reads_the_same_core_facts_as_gix() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());

    let facts = capture_via_git(dir.path()).expect("fallback reads the repo");
    assert_eq!(facts.branch.as_deref(), Some("main"));
    assert!(!facts.detached);
    assert_eq!(facts.head_commit.as_deref().map(str::len), Some(40));
    assert_eq!(facts.is_dirty, None, "dirtiness is exclusive to gix");
    assert_eq!(facts.root_commits.len(), 1);
    assert!(facts.workdir.is_some());

    // Detached behaves the same via the fallback.
    git(dir.path(), &["checkout", "--detach", "HEAD"]);
    let detached = capture_via_git(dir.path()).unwrap();
    assert!(detached.detached);
    assert_eq!(detached.branch, None);
}

#[test]
fn capture_declines_relative_paths() {
    assert!(capture(Path::new(".")).is_none());
    assert!(capture(Path::new("relative/repo")).is_none());
}

/// `path` as one single-quoted shell word.
fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[test]
fn hardened_fallback_neutralizes_hostile_executable_config() {
    // A space in the path, as in a TMPDIR under "Application Support".
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("hostile repo");
    std::fs::create_dir(&dir).unwrap();
    let dir = dir.as_path();
    init_repo(dir);

    // A hostile fsmonitor hook that runs on `git status` (a read) if honored.
    let sentinel = dir.join("PWNED");
    let evil = dir.join("evil.sh");
    std::fs::write(&evil, format!("#!/bin/sh\ntouch {}\n", sh_quote(&sentinel))).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&evil, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // Git runs core.fsmonitor through the shell: quote the path.
    git(dir, &["config", "core.fsmonitor", &sh_quote(&evil)]);

    // Control: plain git honors the hook, proving the vector is live here.
    let _ = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        sentinel.exists(),
        "control: hostile fsmonitor executes under un-hardened git"
    );
    std::fs::remove_file(&sentinel).unwrap();

    // Hardened fallback: reads still succeed and the hook never executes.
    let facts = capture_via_git(dir).expect("hardened capture still reads the repo");
    assert_eq!(facts.branch.as_deref(), Some("main"));
    assert!(
        !sentinel.exists(),
        "hardened git must neutralize hostile executable config (no helper/hook run)"
    );
}

#[test]
fn linked_worktrees_group_by_common_dir() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main");
    std::fs::create_dir(&main).unwrap();
    init_repo(&main);

    // A linked worktree of the same local repository instance.
    let linked = root.path().join("feature-wt");
    git(
        &main,
        &["worktree", "add", linked.to_str().unwrap(), "-b", "feature"],
    );

    let main_facts = capture(&main).unwrap();
    let linked_facts = capture(&linked).unwrap();

    assert_eq!(
        main_facts.common_dir, linked_facts.common_dir,
        "linked worktrees resolve to one common dir (one repository identity)"
    );
    assert_ne!(
        main_facts.workdir, linked_facts.workdir,
        "but they have distinct worktree roots"
    );
    assert_eq!(linked_facts.branch.as_deref(), Some("feature"));
}

#[test]
fn capture_handles_multiple_remotes_and_commits() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());

    // Add a second commit
    std::fs::write(dir.path().join("file2.txt"), "second commit\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "second"]);

    // Add multiple remotes with credentials
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "upstream",
            "https://token@gitlab.com/group/proj.git",
        ],
    );
    git(
        dir.path(),
        &["remote", "add", "origin", "git@github.com:user/repo.git"],
    );

    let facts = capture(dir.path()).expect("capture succeeds");
    assert_eq!(facts.branch.as_deref(), Some("main"));
    assert_eq!(facts.remotes.len(), 2);
    assert_eq!(
        facts.remotes,
        vec![
            "github.com/user/repo".to_string(),
            "gitlab.com/group/proj".to_string()
        ]
    );
    assert_eq!(facts.root_commits.len(), 1);
    assert!(!facts.history_truncated);
}

#[test]
fn capture_identity_matches_capture_identity_evidence() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());

    git(
        dir.path(),
        &["remote", "add", "origin", "git@github.com:user/repo.git"],
    );

    let full = capture(dir.path()).expect("capture succeeds");
    let identity = capture_identity(dir.path()).expect("capture_identity succeeds");

    assert_eq!(identity.common_dir, full.common_dir);
    assert_eq!(identity.workdir, full.workdir);
    assert_eq!(identity.remotes, full.remotes);
    assert_eq!(identity.root_commits, full.root_commits);
    assert_eq!(identity.history_truncated, full.history_truncated);

    // Declines non-absolute and non-git paths
    assert!(capture_identity(Path::new("relative/path")).is_none());
    let non_git = tempfile::tempdir().unwrap();
    assert!(capture_identity(non_git.path()).is_none());
}

#[test]
#[ignore = "benchmark: measure capture latency on clean and dirty repositories"]
fn bench_capture_latency() {
    let make_repo = |num_files: usize, num_modified: usize| -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        for i in 0..num_files {
            let file_path = dir.path().join(format!("file_{i:04}.txt"));
            std::fs::write(&file_path, format!("content {i}\n")).unwrap();
        }
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "initial"]);
        for i in 0..num_modified {
            let file_path = dir.path().join(format!("file_{i:04}.txt"));
            std::fs::write(&file_path, format!("modified content {i}\n")).unwrap();
        }
        dir
    };

    let measure = |label: &str, dir: &Path| {
        // 2 warmups
        for _ in 0..2 {
            let facts = capture(dir).unwrap();
            std::hint::black_box(facts);
        }
        // 7 runs
        let mut times = Vec::new();
        for _ in 0..7 {
            let start = std::time::Instant::now();
            let facts = capture(dir).unwrap();
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            times.push(elapsed);
            std::hint::black_box(facts);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = times[3];
        println!(
            "{label}: median = {:.2} ms, range = [{:.2} ms .. {:.2} ms]",
            median, times[0], times[6]
        );
    };

    println!("Creating 2000 files clean repo...");
    let clean = make_repo(2000, 0);
    measure("capture clean (2000 files)", clean.path());

    println!("Creating 2000 files dirty repo (100 modified)...");
    let dirty = make_repo(2000, 100);
    measure("capture dirty (2000 files, 100 modified)", dirty.path());
}
