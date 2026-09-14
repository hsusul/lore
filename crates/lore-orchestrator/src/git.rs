//! Minimal git CLI wrapper for worktree management.
//!
//! Every invocation disables hooks and terminal prompts so a repository's own
//! config cannot run code or block on credentials while Lore manages worktrees.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Error, Result};

fn git(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    cmd
}

fn run(dir: &Path, args: &[&str]) -> Result<String> {
    run_ok_codes(dir, args, &[0])
}

/// Like `run_ok_codes`, but never reads more than `max` bytes of stdout. Returns
/// the (possibly cut) output and whether more was available.
fn run_capped(dir: &Path, args: &[&str], ok: &[i32], max: usize) -> Result<(String, bool)> {
    use std::io::Read;
    let mut child = git(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| Error::Git(format!("could not run git: {e}")))?;
    let mut buf = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(limit).read_to_end(&mut buf);
    }
    let cut = buf.len() > max;
    if cut {
        // Stop git instead of letting it block on a full pipe.
        let _ = child.kill();
        let _ = child.wait();
        buf.truncate(max);
    } else {
        let status = child
            .wait()
            .map_err(|e| Error::Git(format!("could not run git: {e}")))?;
        if !status.code().is_some_and(|c| ok.contains(&c)) {
            return Err(Error::Git(format!(
                "git {} failed",
                args.first().copied().unwrap_or_default()
            )));
        }
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if cut {
        // from_utf8_lossy may have replaced a split trailing character; fine.
        while text.len() > max {
            text.pop();
        }
    }
    Ok((text, cut))
}

fn run_ok_codes(dir: &Path, args: &[&str], ok: &[i32]) -> Result<String> {
    let out = git(dir)
        .args(args)
        .output()
        .map_err(|e| Error::Git(format!("could not run git: {e}")))?;
    if !out.status.code().is_some_and(|c| ok.contains(&c)) {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Git(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Resolve the top-level directory of the repository containing `path`.
pub fn toplevel(path: &Path) -> Result<PathBuf> {
    run(path, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// Full SHA of `HEAD` in `repo`.
pub fn head_commit(repo: &Path) -> Result<String> {
    run(repo, &["rev-parse", "--verify", "HEAD^{commit}"])
}

/// Whether a local branch named `branch` exists.
pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    run(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// Create `path` as a new worktree on new branch `branch` starting at `base`.
pub fn add_worktree(repo: &Path, path: &Path, branch: &str, base: &str) -> Result<()> {
    let path = path
        .to_str()
        .ok_or_else(|| Error::Git("worktree path is not valid UTF-8".into()))?;
    run(repo, &["worktree", "add", "-b", branch, path, base]).map(|_| ())
}

/// Remove the registered worktree at `path`, discarding its changes.
pub fn remove_worktree(repo: &Path, path: &Path) -> Result<()> {
    let path = path
        .to_str()
        .ok_or_else(|| Error::Git("worktree path is not valid UTF-8".into()))?;
    run(repo, &["worktree", "remove", "--force", path]).map(|_| ())
}

/// Drop metadata for worktrees whose directories no longer exist.
pub fn prune_worktrees(repo: &Path) -> Result<()> {
    run(repo, &["worktree", "prune"]).map(|_| ())
}

/// Delete local branch `branch`, even if unmerged.
pub fn delete_branch(repo: &Path, branch: &str) -> Result<()> {
    if branch_exists(repo, branch) {
        run(repo, &["branch", "-D", branch])?;
    }
    Ok(())
}

/// Commits reachable from `HEAD` in `worktree` but not from `base`.
pub fn commits_ahead(worktree: &Path, base: &str) -> i64 {
    run(worktree, &["rev-list", "--count", &format!("{base}..HEAD")])
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Paths changed relative to `base`: committed, staged, unstaged, and untracked.
/// Returns at most `cap` paths plus the total count.
pub fn changed_files(worktree: &Path, base: &str, cap: usize) -> (Vec<String>, usize) {
    let mut files: Vec<String> = Vec::new();
    for args in [
        &["diff", "--name-only", "-z", base][..],
        &["ls-files", "-z", "--others", "--exclude-standard"][..],
    ] {
        if let Ok(out) = run(worktree, args) {
            files.extend(out.split('\0').map(str::to_string));
        }
    }
    files.retain(|f| !f.is_empty());
    files.sort();
    files.dedup();
    let total = files.len();
    files.truncate(cap);
    (files, total)
}

/// Unified diff of `worktree` against `base`, including untracked files as
/// additions. Never buffers more than `max_bytes` of git output.
pub fn diff_against(worktree: &Path, base: &str, max_bytes: usize) -> Result<(String, bool)> {
    let (mut text, mut truncated) = run_capped(
        worktree,
        &["diff", "--no-color", "--no-ext-diff", base, "--"],
        &[0],
        max_bytes,
    )?;
    let untracked = run(
        worktree,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )?;
    for file in untracked.split('\0').filter(|f| !f.is_empty()) {
        let remaining = max_bytes.saturating_sub(text.len() + 1);
        if truncated || remaining == 0 {
            truncated = true;
            break;
        }
        // `--no-index` exits 1 when the files differ, which is always here.
        if let Ok((part, cut)) = run_capped(
            worktree,
            &["diff", "--no-color", "--no-index", "--", "/dev/null", file],
            &[0, 1],
            remaining,
        ) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&part);
            truncated |= cut;
        }
    }
    Ok((text, truncated))
}

/// `git log --oneline base..HEAD`, newest first, capped.
pub fn log_oneline(worktree: &Path, base: &str, max: usize) -> Vec<String> {
    run(
        worktree,
        &[
            "log",
            "--oneline",
            "--no-decorate",
            &format!("-{max}"),
            &format!("{base}..HEAD"),
        ],
    )
    .map(|s| s.lines().map(str::to_string).collect())
    .unwrap_or_default()
}

/// Paths with uncommitted changes (tracked or untracked).
pub fn uncommitted(worktree: &Path) -> Vec<String> {
    let Ok(out) = run(
        worktree,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    ) else {
        return Vec::new();
    };
    parse_porcelain_z(&out)
}

/// Parse `status --porcelain=v1 -z`. A rename or copy entry (`R`/`C`) is
/// followed by an extra NUL-separated field holding the original path.
fn parse_porcelain_z(out: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut fields = out.split('\0');
    while let Some(entry) = fields.next() {
        let (Some(status), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        if !path.is_empty() {
            paths.push(path.to_string());
        }
        if status.contains('R') || status.contains('C') {
            fields.next();
        }
    }
    paths
}

/// Commits on `branch` since `base`.
pub fn branch_commits_ahead(repo: &Path, base: &str, branch: &str) -> i64 {
    run(
        repo,
        &[
            "rev-list",
            "--count",
            &format!("{base}..refs/heads/{branch}"),
        ],
    )
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(0)
}

/// The branch checked out in `worktree`, if any.
pub fn checked_out_branch(worktree: &Path) -> Option<String> {
    run(worktree, &["symbolic-ref", "--quiet", "HEAD"])
        .ok()
        .and_then(|r| r.strip_prefix("refs/heads/").map(str::to_string))
}

/// Whether a committer identity is configured for `repo`.
pub fn has_identity(repo: &Path) -> bool {
    run(repo, &["var", "GIT_COMMITTER_IDENT"]).is_ok()
}

/// Name of the checked-out branch, or an error when HEAD is detached.
pub fn current_branch(repo: &Path) -> Result<String> {
    run(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map_err(|_| Error::Invalid("the repository is not on a branch (detached HEAD)".into()))
}

/// Stage everything in `worktree` and commit it. Returns false when there was
/// nothing to commit.
pub fn commit_all(worktree: &Path, message: &str) -> Result<bool> {
    if uncommitted(worktree).is_empty() {
        return Ok(false);
    }
    run(worktree, &["add", "-A"])?;
    run(worktree, &["commit", "--no-verify", "-q", "-m", message])?;
    Ok(true)
}

/// Merge `branch` into the branch checked out at `repo`.
///
/// On any failure the checkout is restored to its pre-merge HEAD with a clean
/// index and tree (`merge --abort`, falling back to `reset --merge`). Conflicts
/// are returned as data; other failures as errors.
pub fn merge_branch(
    repo: &Path,
    branch: &str,
    message: &str,
) -> Result<std::result::Result<(), Vec<String>>> {
    let head_before = run(repo, &["rev-parse", "HEAD"])?;
    // Hooks and fsmonitor are off (see `git`). Repository-configured filters and
    // merge drivers still apply; that trust boundary is documented in SECURITY.md.
    let result = run(
        repo,
        &["merge", "--no-ff", "--no-verify", "-m", message, branch],
    );
    match result {
        Ok(_) => {
            if run(repo, &["rev-parse", "HEAD"])? == head_before {
                return Err(Error::Git("git reported nothing to merge".into()));
            }
            Ok(Ok(()))
        }
        Err(err) => {
            let conflicts: Vec<String> = run(repo, &["diff", "--name-only", "--diff-filter=U"])
                .map(|s| s.lines().map(str::to_string).collect())
                .unwrap_or_default();
            let head_now = run(repo, &["rev-parse", "HEAD"]).ok();
            // Only undo what this merge did: if HEAD moved, something else
            // committed and resetting would discard it.
            if head_now.as_deref() == Some(head_before.as_str())
                && run(repo, &["merge", "--abort"]).is_err()
            {
                let _ = run(repo, &["reset", "--merge", &head_before]);
            }
            let restored = run(repo, &["rev-parse", "HEAD"]).ok().as_deref()
                == Some(head_before.as_str())
                && uncommitted(repo).is_empty();
            if !restored {
                return Err(Error::Git(format!(
                    "the merge failed and your checkout could not be fully restored; check `git status` ({err})"
                )));
            }
            if conflicts.is_empty() {
                Err(err)
            } else {
                Ok(Err(conflicts))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_porcelain_z;

    #[test]
    fn porcelain_handles_renames_and_non_ascii() {
        let out = "R  x\0\u{e9}\u{e9}.txt\0?? new.txt\0 M src/a.rs\0";
        assert_eq!(parse_porcelain_z(out), ["x", "new.txt", "src/a.rs"]);
        assert!(parse_porcelain_z("\u{e9}\0").is_empty());
    }
}
