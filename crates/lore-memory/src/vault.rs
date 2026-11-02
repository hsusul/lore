//! The vault: a git repository of note files, and the only place notes live.
//!
//! Layout: `global/<slug>.md` and `repos/<repo name>-<repo key>/<slug>.md`
//! (`repos/<repo key>/` when a note does not name its repository). `.lore/` is
//! Lore's private directory, and is gitignored with `_inbox/` and the
//! `.*.md.tmp` files of an interrupted write. Every write writes one file and
//! commits only that file, so the history stays one note per commit.
//!
//! Lore's commits skip the person's git hooks and commit signing and never
//! prompt, so a hook or a pinentry can never hang a write.

use std::cell::OnceCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use thiserror::Error;

use crate::note::{Note, NoteDate, NoteError, NoteId, RelPath};

/// Lore's private directory inside the vault. Gitignored.
pub const LORE_DIR: &str = ".lore";
const IGNORED: [&str; 3] = [".lore/", "_inbox/", ".*.md.tmp"];
/// Settings every git call carries: no hooks, no signing, no prompts.
const GIT_SAFE: [&str; 8] = [
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "commit.gpgsign=false",
    "-c",
    "tag.gpgsign=false",
    "-c",
    "core.fsmonitor=false",
];

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("no vault at {0}")]
    Missing(PathBuf),
    #[error(transparent)]
    Note(#[from] NoteError),
    #[error("`{0}` is not a valid note path: {1}")]
    BadPath(String, String),
    #[error("a different note already lives at {0}")]
    Conflict(String),
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

/// What `add` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    /// The note was written and committed.
    Committed,
    /// The same note was already in the vault. Nothing was written.
    Unchanged,
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

    /// Add a note from its file text. `id`, `created`, `status`, and `scope`
    /// may be left out; a new note also gets its title as an Obsidian alias.
    /// Adding the same text twice writes and commits once.
    ///
    /// Nothing is written unless the whole note is valid.
    pub fn add(&self, text: &str) -> Result<(Note, Added), VaultError> {
        let draft = Note::parse_draft(text, NoteId::generate(), NoteDate::today())?;
        let mut note = draft.note;
        let rel = note.rel_path();
        let path = self.root.join(&rel);

        let existing = match fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(io_error(format!("read {rel}"))(e)),
        };
        if let Some(existing) = existing {
            let current = Note::parse(&existing).map_err(|_| VaultError::Conflict(rel.clone()))?;
            if draft.id_filled {
                note.id = current.id;
            }
            if draft.created_filled {
                note.created = current.created;
            }
            if note.render() != existing {
                return Err(VaultError::Conflict(rel));
            }
            if self.is_committed(&rel)? {
                return Ok((note, Added::Unchanged));
            }
        } else {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(io_error(format!("create {rel}")))?;
            }
            write_atomically(&path, &note.render())?;
        }
        self.commit(&rel, &format!("mem: add {}", note.slug()))?;
        Ok((note, Added::Committed))
    }

    /// Stage and commit one file.
    fn commit(&self, rel: &str, message: &str) -> Result<(), VaultError> {
        self.git(&["add", "--", rel])?;
        self.git(&["commit", "-q", "-m", message, "--", rel])?;
        Ok(())
    }

    /// Read a note file by its vault-relative path.
    pub fn read(&self, rel: &str) -> Result<String, VaultError> {
        let rel =
            RelPath::parse(rel).map_err(|e| VaultError::BadPath(rel.into(), e.to_string()))?;
        fs::read_to_string(self.root.join(rel.as_str()))
            .map_err(io_error(format!("read {}", rel.as_str())))
    }

    fn is_committed(&self, rel: &str) -> Result<bool, VaultError> {
        let status = self.git(&["status", "--porcelain", "--", rel])?;
        Ok(status.trim().is_empty())
    }

    fn git(&self, args: &[&str]) -> Result<String, VaultError> {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.root).args(GIT_SAFE);
        let (has_name, has_email) = self.identity();
        if !has_name {
            command.args(["-c", "user.name=Lore"]);
        }
        if !has_email {
            command.args(["-c", "user.email=lore@localhost"]);
        }
        let output = command
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
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
                .env("GIT_TERMINAL_PROMPT", "0")
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
