//! The scratch repository a replay runs in.
//!
//! A replay must see the repository as it was when the prompt was given, and
//! must never write to the person's real one. So it fetches the commit HEAD
//! pointed at then (from the reflog, via [`lore_core::git::head_at`]) into a
//! new repository and checks it out. It fetches that commit alone, not a
//! clone: a clone brings every branch, tag, and object, so the agent could
//! read the later fix it is being graded against. Nor is it `--shared`: that
//! borrows the original's object store, which the sandbox hides.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::error::{Error, Result};
use crate::types::{Case, Fidelity};

/// Where a case's repository can be reproduced from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The repository and the commit to check out.
    Commit { toplevel: PathBuf, commit: String },
    /// No commit; the replay runs in an empty folder. The toplevel is kept
    /// when known, so paths inside the repository still count as its own.
    Unavailable {
        fidelity: Fidelity,
        toplevel: Option<PathBuf>,
    },
}

impl Source {
    #[must_use]
    pub fn toplevel(&self) -> Option<&Path> {
        match self {
            Self::Commit { toplevel, .. } => Some(toplevel),
            Self::Unavailable { toplevel, .. } => toplevel.as_deref(),
        }
    }

    #[must_use]
    pub fn fidelity(&self) -> Fidelity {
        match self {
            Self::Commit { commit, .. } => Fidelity::CommitOnly {
                commit: commit.clone(),
            },
            Self::Unavailable { fidelity, .. } => fidelity.clone(),
        }
    }
}

/// Decide what the case's repository can be reproduced from. Reads the
/// original repository; writes nothing.
#[must_use]
pub fn source(case: &Case) -> Source {
    let unavailable = |fidelity, toplevel| Source::Unavailable { fidelity, toplevel };
    let Some(cwd) = case.cwd.as_deref() else {
        return unavailable(Fidelity::NoCwd, None);
    };
    if !cwd.is_dir() {
        return unavailable(Fidelity::CwdGone, None);
    }
    let Some(toplevel) = toplevel(cwd) else {
        return unavailable(Fidelity::NotARepo, None);
    };
    match lore_core::git::head_at(cwd, case.prompt_ms) {
        Some(head) => Source::Commit {
            toplevel,
            commit: head.commit_sha,
        },
        None => unavailable(Fidelity::ReflogTooYoung, Some(toplevel)),
    }
}

fn toplevel(cwd: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let top = String::from_utf8(out.stdout).ok()?;
    let top = top.trim_end_matches(['\n', '\r']);
    (!top.is_empty()).then(|| PathBuf::from(top))
}

/// Make `dest` (which must not exist) a repository holding only `commit` and
/// its history, and check it out, on `branch` when `toplevel` has that
/// branch. The repository has no remote, so the agent cannot push anywhere.
pub fn clone_at(toplevel: &Path, commit: &str, branch: Option<&str>, dest: &Path) -> Result<()> {
    git(None, &[os("init"), os("-q"), dest.into()])?;
    // Protocol v2 serves any commit by its id; v0 refuses one no ref points
    // at, which is most of them once the branch has moved on.
    git(
        Some(dest),
        &[
            os("-c"),
            os("protocol.version=2"),
            os("fetch"),
            os("-q"),
            os("--no-tags"),
            toplevel.into(),
            os(commit),
        ],
    )?;
    git(
        Some(dest),
        &[os("checkout"), os("-q"), os("--detach"), os(commit)],
    )?;
    if let Some(branch) = branch {
        let reference = format!("refs/heads/{branch}");
        let exists = run(
            Some(toplevel),
            &[os("rev-parse"), os("-q"), os("--verify"), os(&reference)],
        )
        .is_ok_and(|o| o.status.success());
        if exists {
            let _ = run(
                Some(dest),
                &[os("checkout"), os("-q"), os("-B"), os(branch), os(commit)],
            );
        }
    }
    Ok(())
}

fn os(s: &str) -> std::ffi::OsString {
    s.into()
}

fn run(dir: Option<&Path>, args: &[std::ffi::OsString]) -> std::io::Result<Output> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args).output()
}

fn git(dir: Option<&Path>, args: &[std::ffi::OsString]) -> Result<()> {
    let describe = || {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let out = run(dir, args).map_err(|e| Error::Git {
        args: describe(),
        stderr: e.to_string(),
    })?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Git {
            args: describe(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Signal;

    fn case(cwd: Option<PathBuf>, prompt_ms: i64) -> Case {
        Case {
            id: "s:1".into(),
            agent: crate::types::Agent::ClaudeCode,
            lesson: "l".into(),
            session: "s".into(),
            native: "n".into(),
            model: None,
            file: PathBuf::from("/nonexistent"),
            prompt_line: 0,
            correction_line: 1,
            correction_ms: prompt_ms,
            cwd,
            branch: None,
            prompt_ms,
            prompt: String::new(),
            actions: Vec::new(),
            correction: String::new(),
            repo: None,
            signal: Signal::Correction,
        }
    }

    fn sh(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn fidelity_follows_what_the_cwd_still_offers() {
        let tmp = tempfile::tempdir().unwrap();
        let now = 4_000_000_000_000;
        assert_eq!(source(&case(None, now)).fidelity(), Fidelity::NoCwd);
        let gone = tmp.path().join("gone");
        assert_eq!(source(&case(Some(gone), now)).fidelity(), Fidelity::CwdGone);
        let plain = tmp.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        assert_eq!(
            source(&case(Some(plain), now)).fidelity(),
            Fidelity::NotARepo
        );

        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        sh(&repo, &["init", "-q", "-b", "main"]);
        sh(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let young = source(&case(Some(repo.clone()), 1_000));
        assert_eq!(young.fidelity(), Fidelity::ReflogTooYoung);
        assert!(young.toplevel().is_some());
        let Source::Commit { commit, .. } = source(&case(Some(repo), now)) else {
            panic!("expected a commit");
        };
        assert_eq!(commit.len(), 40);
    }

    fn out(dir: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
    }

    fn stdout(dir: &Path, args: &[&str]) -> String {
        let o = out(dir, args);
        assert!(o.status.success(), "git {args:?}");
        String::from_utf8(o.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn the_clone_holds_no_history_after_the_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        sh(&repo, &["init", "-q", "-b", "main"]);
        sh(&repo, &["commit", "-q", "--allow-empty", "-m", "then"]);
        let then = stdout(&repo, &["rev-parse", "HEAD"]);
        sh(&repo, &["commit", "-q", "--allow-empty", "-m", "later"]);
        sh(&repo, &["tag", "v2"]);
        sh(&repo, &["checkout", "-q", "-b", "fix"]);
        sh(&repo, &["commit", "-q", "--allow-empty", "-m", "the fix"]);
        let later = stdout(&repo, &["rev-list", "--all"]);

        let dest = tmp.path().join("clone");
        clone_at(&repo, &then, Some("main"), &dest).unwrap();

        assert_eq!(stdout(&dest, &["rev-parse", "HEAD"]), then);
        assert_eq!(stdout(&dest, &["symbolic-ref", "HEAD"]), "refs/heads/main");
        assert_eq!(
            stdout(&dest, &["for-each-ref", "--format=%(refname)"]),
            "refs/heads/main"
        );
        assert_eq!(
            stdout(&dest, &["rev-list", "--all", "--reflog"]),
            then,
            "a ref or reflog reaches past the commit"
        );
        for sha in later.lines().filter(|sha| *sha != then) {
            assert!(
                !out(&dest, &["cat-file", "-e", sha]).status.success(),
                "{sha} from after the commit is in the clone"
            );
        }
        assert_eq!(stdout(&dest, &["remote"]), "");
    }
}
