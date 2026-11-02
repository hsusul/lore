//! Which repository a directory belongs to.
//!
//! A repository's identity is the hash of its normalized `origin` remote, so
//! every clone and worktree of one repository shares one [`RepoKey`]. The
//! normalization is `lore-core`'s, which drops credentials and makes SSH and
//! HTTPS forms of one remote equal.
//!
//! Discovery reads the repository in process with `gix`, because recall runs on
//! every query and a `git` spawn costs about as much as the search itself.

use std::path::{Path, PathBuf};

use crate::note::{RepoKey, RepoName};

/// Hex characters of the remote hash kept in a [`RepoKey`].
const KEY_LEN: usize = 16;

/// What a directory is, as far as note scope is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoContext {
    /// Not inside a git working tree.
    NoRepo,
    /// A working tree with no usable `origin` remote.
    NoRemote { root: PathBuf },
    /// A working tree whose notes live under `repo:<key>`. `name` is the
    /// origin's last path segment, for the note's `repo:` and its folder.
    Repo {
        root: PathBuf,
        key: RepoKey,
        name: Option<RepoName>,
    },
}

impl RepoContext {
    /// Resolve the working tree that contains `dir`. A bare repository has
    /// none, so it is `NoRepo`.
    #[must_use]
    pub fn discover(dir: &Path) -> Self {
        let Ok(repo) = gix::discover(dir) else {
            return RepoContext::NoRepo;
        };
        let Some(workdir) = repo.workdir() else {
            return RepoContext::NoRepo;
        };
        let root = workdir
            .canonicalize()
            .unwrap_or_else(|_| workdir.to_path_buf());
        let url = repo.find_remote("origin").ok().and_then(|remote| {
            remote
                .url(gix::remote::Direction::Fetch)
                .map(|url| url.to_bstring().to_string())
        });
        match url.as_deref().and_then(|url| Some((repo_key(url)?, url))) {
            Some((key, url)) => RepoContext::Repo {
                root,
                key,
                name: RepoName::from_origin(url),
            },
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

    /// The repository's display name, when it has an `origin` remote whose
    /// URL ends in one.
    #[must_use]
    pub fn name(&self) -> Option<&RepoName> {
        match self {
            RepoContext::Repo { name, .. } => name.as_ref(),
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
