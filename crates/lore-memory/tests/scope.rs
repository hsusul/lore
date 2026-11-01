//! Repo identity: every form of one remote, every clone, and every worktree of
//! one repository share one scope.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{commit, git, repo};
use lore_memory::repo::{repo_key, RepoContext};

/// The first 16 hex characters of BLAKE3(`github.com/hsusul/lore`).
const KEY: &str = "81444c90f1ff44ac";

#[test]
fn scope_is_the_same_for_ssh_and_https_remotes() {
    for remote in [
        "git@github.com:hsusul/lore.git",
        "ssh://git@github.com/hsusul/lore",
        "https://github.com/hsusul/lore.git",
        "https://user:token@GitHub.com/hsusul/lore/",
    ] {
        assert_eq!(repo_key(remote).unwrap().as_str(), KEY, "{remote}");
    }
    assert_ne!(
        repo_key("git@github.com:hsusul/other.git")
            .unwrap()
            .as_str(),
        KEY
    );
    assert_eq!(repo_key(""), None);
}

#[test]
fn scope_is_shared_by_clones_and_worktrees() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join("ssh");
    let https = dir.path().join("https");
    repo(&ssh, Some("git@github.com:hsusul/lore.git"));
    repo(&https, Some("https://github.com/hsusul/lore"));
    commit(&ssh, "a.txt", "a\n", "a");
    let worktree = dir.path().join("wt");
    git(
        &ssh,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            worktree.to_str().unwrap(),
        ],
    );
    std::fs::create_dir_all(worktree.join("deep/er")).unwrap();

    for cwd in [&ssh, &https, &worktree, &worktree.join("deep/er")] {
        let context = RepoContext::discover(cwd);
        assert_eq!(context.key().map(|k| k.as_str()), Some(KEY), "{cwd:?}");
    }
}

#[test]
fn scope_without_a_remote_or_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("no-remote");
    repo(&bare, None);
    assert_eq!(
        RepoContext::discover(&bare),
        RepoContext::NoRemote {
            root: bare.canonicalize().unwrap()
        }
    );
    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(RepoContext::discover(&plain), RepoContext::NoRepo);
}
