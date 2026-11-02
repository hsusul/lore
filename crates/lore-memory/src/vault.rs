//! The vault: a git repository of note files, and the only place notes live.
//!
//! `.lore/` is Lore's private directory inside the vault. It is gitignored with
//! `_inbox/` and the `.*.md.tmp` files of an interrupted write.

use std::cell::OnceCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use thiserror::Error;

/// Lore's private directory inside the vault. Gitignored.
pub const LORE_DIR: &str = ".lore";
const IGNORED: [&str; 3] = [".lore/", "_inbox/", ".*.md.tmp"];

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("no vault at {0}")]
    Missing(PathBuf),
    #[error("{0}: {1}")]
    Io(String, #[source] io::Error),
    #[error("`git {0}` failed: {1}")]
    Git(String, String),
}

fn io_error(what: impl Into<String>) -> impl FnOnce(io::Error) -> VaultError {
    let what = what.into();
    move |error| VaultError::Io(what, error)
}

/// What `init` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Init {
    Created,
    AlreadyInitialized,
}

/// A vault on disk.
#[derive(Debug)]
pub struct Vault {
    root: PathBuf,
    /// Whether git has a `user.name` and a `user.email`.
    identity: OnceCell<(bool, bool)>,
}

impl Vault {
    /// Open an existing vault. Fails with [`VaultError::Missing`] when `root`
    /// is not a git repository.
    pub fn open(root: &Path) -> Result<Self, VaultError> {
        if root.join(".git").exists() {
            Ok(Self::at(root))
        } else {
            Err(VaultError::Missing(root.to_path_buf()))
        }
    }

    /// Create the vault, or finish creating it. Safe to run again.
    pub fn init(root: &Path) -> Result<(Self, Init), VaultError> {
        let vault = Self::at(root);
        fs::create_dir_all(vault.lore_dir()).map_err(io_error("create the vault"))?;
        let fresh_repo = !root.join(".git").exists();
        if fresh_repo {
            vault.git(&["init", "-q"])?;
        }
        let ignore_path = root.join(".gitignore");
        let mut ignore = match fs::read_to_string(&ignore_path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io_error("read .gitignore")(e)),
        };
        let missing: Vec<&str> = IGNORED
            .into_iter()
            .filter(|entry| !ignore.lines().any(|line| line.trim() == *entry))
            .collect();
        if missing.is_empty() && !fresh_repo {
            return Ok((vault, Init::AlreadyInitialized));
        }
        if !ignore.is_empty() && !ignore.ends_with('\n') {
            ignore.push('\n');
        }
        for entry in missing {
            ignore.push_str(entry);
            ignore.push('\n');
        }
        write_atomically(&ignore_path, &ignore)?;
        vault.git(&["add", "--", ".gitignore"])?;
        vault.git(&["commit", "-q", "-m", "mem: init vault", "--", ".gitignore"])?;
        Ok((vault, Init::Created))
    }

    fn at(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            identity: OnceCell::new(),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<vault>/.lore`, Lore's private directory.
    #[must_use]
    pub fn lore_dir(&self) -> PathBuf {
        self.root.join(LORE_DIR)
    }

    fn git(&self, args: &[&str]) -> Result<String, VaultError> {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.root);
        let (has_name, has_email) = self.identity();
        if !has_name {
            command.args(["-c", "user.name=Lore"]);
        }
        if !has_email {
            command.args(["-c", "user.email=lore@localhost"]);
        }
        let output = command
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(io_error("run git"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(VaultError::Git(
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    /// Whether git has a `user.name` and a `user.email`. A person's own
    /// identity is used when set; a fallback is passed only for what is missing.
    fn identity(&self) -> (bool, bool) {
        *self.identity.get_or_init(|| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["config", "--get-regexp", r"^user\.(name|email)$"])
                .stdin(Stdio::null())
                .output();
            let text = output
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            let has = |key: &str| {
                text.lines().any(|line| {
                    line.split_once(' ')
                        .is_some_and(|(k, v)| k == key && !v.trim().is_empty())
                })
            };
            (has("user.name"), has("user.email"))
        })
    }
}

/// Write through a dot-prefixed temporary file and a rename, so a reader never
/// sees half a note.
fn write_atomically(path: &Path, text: &str) -> Result<(), VaultError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.tmp"));
    fs::write(&temp, text).map_err(io_error(format!("write {}", temp.display())))?;
    fs::rename(&temp, path).map_err(io_error(format!("write {}", path.display())))
}
