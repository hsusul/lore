//! The vault: a git repository of note files, and the only place notes live.
//!
//! Layout: `global/<slug>.md` and `repos/<repo key>/<slug>.md`. `.lore/` holds
//! the write lock and the index, and is gitignored with `_inbox/`. Every write
//! takes `.lore/write.lock`, writes one file, and commits only that file, so two
//! writers queue rather than interleave and the history stays one note per
//! commit.

use std::cell::OnceCell;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use thiserror::Error;

use crate::note::{Note, NoteDate, NoteError, NoteId, RelPath};

/// Environment variable that overrides the vault location.
pub const VAULT_ENV: &str = "LORE_VAULT_DIR";
/// The default vault directory name, under the home directory.
pub const DEFAULT_DIR_NAME: &str = "Lore";
/// Lore's private directory inside the vault. Gitignored.
pub const LORE_DIR: &str = ".lore";
const LOCK_FILE: &str = "write.lock";
const IGNORED: [&str; 2] = [".lore/", "_inbox/"];

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("the vault location must be an absolute path, got `{0}`")]
    RelativeDir(PathBuf),
    #[error("no home directory to put the default vault in; set {VAULT_ENV}")]
    NoHome,
    #[error("no vault at {0}")]
    Missing(PathBuf),
    #[error(transparent)]
    Note(#[from] NoteError),
    #[error("`{0}` is not a valid note path: {1}")]
    BadPath(String, String),
    #[error("a different note already lives at {0}")]
    Conflict(String),
    #[error("no note matches `{0}`")]
    NotFound(String),
    #[error("`{0}` matches more than one note: {1}")]
    Ambiguous(String, String),
    #[error("{0}: {1}")]
    Io(String, #[source] io::Error),
    #[error("`git {0}` failed: {1}")]
    Git(String, String),
}

fn io_error(what: impl Into<String>) -> impl FnOnce(io::Error) -> VaultError {
    let what = what.into();
    move |error| VaultError::Io(what, error)
}

/// Where the vault is: `flag`, else `$LORE_VAULT_DIR`, else `~/Lore`.
pub fn resolve_dir(flag: Option<&Path>) -> Result<PathBuf, VaultError> {
    let env = std::env::var_os(VAULT_ENV).filter(|v| !v.is_empty());
    let dir = match (flag, env) {
        (Some(flag), _) => flag.to_path_buf(),
        (None, Some(env)) => PathBuf::from(env),
        (None, None) => dirs::home_dir()
            .ok_or(VaultError::NoHome)?
            .join(DEFAULT_DIR_NAME),
    };
    if dir.is_absolute() {
        Ok(dir)
    } else {
        Err(VaultError::RelativeDir(dir))
    }
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

/// A markdown file found in the vault, and whether it is a valid note.
#[derive(Debug)]
pub struct VaultFile {
    pub rel: String,
    pub note: Result<Note, NoteError>,
}

/// An exclusive hold on the vault's write lock. Released on drop.
#[derive(Debug)]
pub struct WriteLock {
    _file: File,
}

/// A vault on disk.
#[derive(Debug)]
pub struct Vault {
    root: PathBuf,
    has_identity: OnceCell<bool>,
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
        let _lock = vault.lock()?;
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
            has_identity: OnceCell::new(),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<vault>/.lore`, where the lock and the index live.
    #[must_use]
    pub fn lore_dir(&self) -> PathBuf {
        self.root.join(LORE_DIR)
    }

    /// Wait for, then hold, the vault's write lock.
    pub fn lock(&self) -> Result<WriteLock, VaultError> {
        fs::create_dir_all(self.lore_dir()).map_err(io_error("create .lore"))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.lore_dir().join(LOCK_FILE))
            .map_err(io_error("open the write lock"))?;
        file.lock().map_err(io_error("take the write lock"))?;
        Ok(WriteLock { _file: file })
    }

    /// Add a note from its file text. `id`, `created`, `status`, and `scope`
    /// may be left out. Adding the same text twice writes and commits once.
    ///
    /// Nothing is written unless the whole note is valid.
    pub fn add(&self, text: &str) -> Result<(Note, Added), VaultError> {
        let draft = Note::parse_draft(text, NoteId::generate(), NoteDate::today())?;
        let mut note = draft.note;
        let rel = note.rel_path();
        let path = self.root.join(&rel);
        let _lock = self.lock()?;

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
        let message = format!("mem: add {}", note.slug());
        self.git(&["add", "--", &rel])?;
        self.git(&["commit", "-q", "-m", &message, "--", &rel])?;
        Ok((note, Added::Committed))
    }

    /// Read a note file by its vault-relative path.
    pub fn read(&self, rel: &str) -> Result<String, VaultError> {
        let rel =
            RelPath::parse(rel).map_err(|e| VaultError::BadPath(rel.into(), e.to_string()))?;
        fs::read_to_string(self.root.join(rel.as_str()))
            .map_err(io_error(format!("read {}", rel.as_str())))
    }

    /// Every `.md` file in the vault outside `.lore`, `.git`, other dot
    /// directories, and `_inbox`, sorted by path, each with its parse result.
    pub fn notes(&self) -> Result<Vec<VaultFile>, VaultError> {
        let mut files = Vec::new();
        collect_markdown(&self.root, &self.root, &mut files)?;
        files.sort();
        files
            .into_iter()
            .map(|rel| {
                let text = fs::read_to_string(self.root.join(&rel))
                    .map_err(io_error(format!("read {rel}")))?;
                let note = Note::parse(&text);
                Ok(VaultFile { rel, note })
            })
            .collect()
    }

    fn is_committed(&self, rel: &str) -> Result<bool, VaultError> {
        let status = self.git(&["status", "--porcelain", "--", rel])?;
        Ok(status.trim().is_empty())
    }

    fn git(&self, args: &[&str]) -> Result<String, VaultError> {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.root);
        if !self.identity_configured() {
            command.args(["-c", "user.name=Lore", "-c", "user.email=lore@localhost"]);
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

    /// Whether git has a committer email. A person's own identity is used when
    /// set; a fallback is passed only when there is none.
    fn identity_configured(&self) -> bool {
        *self.has_identity.get_or_init(|| {
            Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["config", "user.email"])
                .stdin(Stdio::null())
                .output()
                .is_ok_and(|o| o.status.success() && !o.stdout.trim_ascii().is_empty())
        })
    }
}

fn collect_markdown(dir: &Path, root: &Path, out: &mut Vec<String>) -> Result<(), VaultError> {
    let entries = fs::read_dir(dir).map_err(io_error(format!("list {}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(io_error(format!("list {}", dir.display())))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || (dir == root && name == "_inbox") {
            continue;
        }
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(io_error(format!("stat {}", path.display())))?;
        if kind.is_dir() {
            collect_markdown(&path, root, out)?;
        } else if kind.is_file() && name.ends_with(".md") {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
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
