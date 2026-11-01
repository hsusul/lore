//! Which repository a directory belongs to, and the `git` calls that ask.
//!
//! A repository's identity is the hash of its normalized `origin` remote, so
//! every clone and worktree of one repository shares one [`RepoKey`]. The
//! normalization is `lore-core`'s, which drops credentials and makes SSH and
//! HTTPS forms of one remote equal.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::note::RepoKey;

/// Hex characters of the remote hash kept in a [`RepoKey`].
const KEY_LEN: usize = 16;

/// Variables that would point `git` somewhere other than the directory asked.
const REDIRECTING_ENV: [&str; 5] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

/// What a directory is, as far as note scope is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoContext {
    /// Not inside a git working tree.
    NoRepo,
    /// A working tree with no usable `origin` remote.
    NoRemote { root: PathBuf },
    /// A working tree whose notes live under `repo:<key>`.
    Repo { root: PathBuf, key: RepoKey },
}

impl RepoContext {
    /// Resolve the repository that contains `dir`.
    #[must_use]
    pub fn discover(dir: &Path) -> Self {
        let Some(root) = git(dir, ["rev-parse", "--show-toplevel"]) else {
            return RepoContext::NoRepo;
        };
        let root = PathBuf::from(root.trim_end_matches(['\n', '\r']));
        let key = git(&root, ["config", "--get", "remote.origin.url"])
            .and_then(|url| repo_key(url.trim()));
        match key {
            Some(key) => RepoContext::Repo { root, key },
            None => RepoContext::NoRemote { root },
        }
    }

    /// The working tree root, when there is one.
    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        match self {
            RepoContext::NoRepo => None,
            RepoContext::NoRemote { root } | RepoContext::Repo { root, .. } => Some(root),
        }
    }

    /// The repository's key, when it has an `origin` remote.
    #[must_use]
    pub fn key(&self) -> Option<&RepoKey> {
        match self {
            RepoContext::Repo { key, .. } => Some(key),
            RepoContext::NoRepo | RepoContext::NoRemote { .. } => None,
        }
    }
}

/// The key for a remote URL in any form git accepts. `None` when the URL does
/// not normalize.
#[must_use]
pub fn repo_key(remote_url: &str) -> Option<RepoKey> {
    let normalized = lore_core::git::normalize_remote_url(remote_url)?;
    let hex = blake3::hash(normalized.as_bytes()).to_hex();
    RepoKey::parse(&hex[..KEY_LEN]).ok()
}

/// Run a read-only `git` command in `dir` and return its stdout, or `None` when
/// git is missing or the command fails.
///
/// Optional locks are off, so a status refresh never writes the caller's index,
/// and variables that would redirect git elsewhere are removed.
pub(crate) fn git<I, S>(dir: &Path, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    for name in REDIRECTING_ENV {
        command.env_remove(name);
    }
    let output = command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}
