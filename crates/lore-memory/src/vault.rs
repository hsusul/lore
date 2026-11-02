//! The vault: a git repository of note files, and the only place notes live.
//!
//! Layout: `global/<slug>.md` and `repos/<repo name>-<repo key>/<slug>.md`
//! (`repos/<repo key>/` when a note does not name its repository). A person may
//! move or rename files; the vault finds notes by walking it. `.lore/` holds the
//! write lock and the index, and is gitignored with `_inbox/` and the
//! `.*.md.tmp` files of an interrupted write. Every write takes
//! `.lore/write.lock` (waiting at most [`LOCK_WAIT`]), writes one file, and
//! commits only that file, so two writers queue rather than interleave and the
//! history stays one note per commit.
//!
//! Lore's commits skip the person's git hooks and commit signing and never
//! prompt, so a hook or a pinentry can never hold the lock forever.

use std::cell::OnceCell;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

use thiserror::Error;

use crate::index::{fingerprint, FileStamp, Hit, Index, IndexError};
use crate::note::{Note, NoteDate, NoteError, NoteId, RelPath};

/// Lore's private directory inside the vault. Gitignored.
pub const LORE_DIR: &str = ".lore";
const LOCK_FILE: &str = "write.lock";
const IGNORED: [&str; 3] = [".lore/", "_inbox/", ".*.md.tmp"];
/// How long a writer waits for another to release the vault lock before it
/// gives up with [`VaultError::Busy`].
pub const LOCK_WAIT: Duration = Duration::from_secs(5);
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
    #[error("id `{id}` is already used by {existing}")]
    DuplicateId { id: String, existing: String },
    #[error("another Lore process is writing to the vault at {}", .0.display())]
    Busy(PathBuf),
    #[error("no note matches `{0}`")]
    NotFound(String),
    #[error("`{0}` matches more than one note: {1}")]
    Ambiguous(String, String),
    #[error("{0}: {1}")]
    Io(String, #[source] io::Error),
    #[error("`git {0}` failed: {1}")]
    Git(String, String),
    #[error(transparent)]
    Index(#[from] IndexError),
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

/// What `reindex` read.
#[derive(Debug)]
pub struct Reindexed {
    pub indexed: usize,
    /// Markdown files that are not valid notes, with the reason.
    pub skipped: Vec<(String, NoteError)>,
    /// Ids that more than one file carries. Every such file is still indexed.
    pub duplicates: Vec<DuplicateId>,
}

/// One id carried by several note files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateId {
    pub id: NoteId,
    /// The files, sorted.
    pub paths: Vec<String>,
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
            identity: OnceCell::new(),
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

    /// Hold the vault's write lock, waiting at most [`LOCK_WAIT`] for another
    /// writer to release it.
    pub fn lock(&self) -> Result<WriteLock, VaultError> {
        fs::create_dir_all(self.lore_dir()).map_err(io_error("create .lore"))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.lore_dir().join(LOCK_FILE))
            .map_err(io_error("open the write lock"))?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(WriteLock { _file: file }),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(TryLockError::WouldBlock) => return Err(VaultError::Busy(self.root.clone())),
                Err(TryLockError::Error(e)) => return Err(io_error("take the write lock")(e)),
            }
        }
    }

    /// Add a note from its file text. `id`, `created`, `status`, and `scope`
    /// may be left out; a new note also gets its title as an Obsidian alias.
    /// Adding the same text twice writes and commits once.
    ///
    /// Nothing is written unless the whole note is valid and its id is not
    /// already used by another file.
    pub fn add(&self, text: &str) -> Result<(Note, Added), VaultError> {
        let draft = Note::parse_draft(text, NoteId::generate(), NoteDate::today())?;
        let mut note = draft.note;
        let rel = note.rel_path();
        let path = self.root.join(&rel);
        let _lock = self.lock()?;
        let index = self.refresh_locked()?;

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
            check_id_free(&index, &note.id, &rel)?;
            if self.is_committed(&rel)? {
                return Ok((note, Added::Unchanged));
            }
        } else {
            check_id_free(&index, &note.id, &rel)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(io_error(format!("create {rel}")))?;
            }
            write_atomically(&path, &note.render())?;
        }
        drop(index);
        self.commit(&rel, &format!("mem: add {}", note.slug()))?;
        Ok((note, Added::Committed))
    }

    /// Write `note` over the note file at `rel` and commit it. The file must
    /// exist and hold the same id; it stays where it is, even when a person
    /// renamed or moved it. Writing what the file already holds is
    /// [`Added::Unchanged`].
    pub fn write(&self, rel: &str, note: &Note) -> Result<Added, VaultError> {
        let rel = RelPath::parse(rel)
            .ok()
            .filter(|r| r.as_str().ends_with(".md"))
            .ok_or_else(|| VaultError::BadPath(rel.into(), "expected a `.md` file".into()))?;
        let rel = rel.as_str();
        let path = self.root.join(rel);
        let _lock = self.lock()?;
        let existing = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(VaultError::NotFound(rel.into()))
            }
            Err(e) => return Err(io_error(format!("read {rel}"))(e)),
        };
        let current = Note::parse(&existing).map_err(|_| VaultError::Conflict(rel.into()))?;
        if current.id != note.id {
            return Err(VaultError::Conflict(rel.into()));
        }
        let index = self.refresh_locked()?;
        check_id_free(&index, &note.id, rel)?;
        drop(index);
        let text = note.render();
        if text == existing {
            if self.is_committed(rel)? {
                return Ok(Added::Unchanged);
            }
        } else {
            write_atomically(&path, &text)?;
        }
        let stem = rel.rsplit('/').next().unwrap_or(rel);
        let stem = stem.strip_suffix(".md").unwrap_or(stem);
        self.commit(rel, &format!("mem: update {stem}"))?;
        Ok(Added::Committed)
    }

    /// Stage and commit one file, then bring the index up to date.
    fn commit(&self, rel: &str, message: &str) -> Result<(), VaultError> {
        self.git(&["add", "--", rel])?;
        self.git(&["commit", "-q", "-m", message, "--", rel])?;
        self.refresh_locked()?;
        Ok(())
    }

    /// Rebuild the index from the vault files alone.
    pub fn reindex(&self) -> Result<Reindexed, VaultError> {
        let _lock = self.lock()?;
        self.build_locked(self.stamps()?)
    }

    fn build_locked(&self, stamps: Vec<FileStamp>) -> Result<Reindexed, VaultError> {
        let mut notes = Vec::new();
        let mut skipped = Vec::new();
        for stamp in &stamps {
            match self.parse_file(&stamp.path)? {
                Ok(note) => notes.push((stamp.path.as_str(), note)),
                Err(error) => skipped.push((stamp.path.clone(), error)),
            }
        }
        let mut by_id: std::collections::BTreeMap<&NoteId, Vec<String>> = Default::default();
        for (rel, note) in &notes {
            by_id.entry(&note.id).or_default().push((*rel).to_string());
        }
        let duplicates = by_id
            .into_iter()
            .filter(|(_, paths)| paths.len() > 1)
            .map(|(id, paths)| DuplicateId {
                id: id.clone(),
                paths,
            })
            .collect();
        let indexed = Index::build(
            &self.lore_dir(),
            notes.iter().map(|(rel, note)| (*rel, note)),
            &stamps,
        )?;
        Ok(Reindexed {
            indexed,
            skipped,
            duplicates,
        })
    }

    /// The index, brought up to date with the vault files first.
    ///
    /// The common case costs one directory walk with a stat per file and one
    /// read of the stored fingerprint. When a person edited, renamed, added, or
    /// deleted a file, only those files are read again, under the write lock.
    pub fn index(&self) -> Result<Index, VaultError> {
        let stamps = self.stamps()?;
        if let Some(index) = Index::open(&self.lore_dir())? {
            if index.fingerprint()?.as_deref() == Some(fingerprint(&stamps).as_str()) {
                return Ok(index);
            }
        }
        let _lock = self.lock()?;
        self.refresh_locked()
    }

    /// [`Vault::index`] for a caller that holds the write lock.
    fn refresh_locked(&self) -> Result<Index, VaultError> {
        let stamps = self.stamps()?;
        let print = fingerprint(&stamps);
        let lore_dir = self.lore_dir();
        let mut index = match Index::open(&lore_dir)? {
            Some(index) => index,
            None => {
                self.build_locked(stamps)?;
                return Index::open(&lore_dir)?
                    .ok_or_else(|| VaultError::Missing(lore_dir.join(crate::index::INDEX_FILE)));
            }
        };
        if index.fingerprint()?.as_deref() == Some(print.as_str()) {
            return Ok(index);
        }
        let mut stored = index.stamps()?;
        let mut changed = Vec::new();
        for stamp in stamps {
            let same = stored
                .remove(&stamp.path)
                .is_some_and(|old| old == (stamp.size, stamp.mtime_ns));
            if !same {
                let note = self.parse_file(&stamp.path)?.ok();
                changed.push((stamp, note));
            }
        }
        let removed: Vec<String> = stored.into_keys().collect();
        index.refresh(&changed, &removed, &print)?;
        Ok(index)
    }

    /// Read and parse one vault file. The outer error is a failed read.
    fn parse_file(&self, rel: &str) -> Result<Result<Note, NoteError>, VaultError> {
        let text =
            fs::read_to_string(self.root.join(rel)).map_err(io_error(format!("read {rel}")))?;
        Ok(Note::parse(&text))
    }

    /// A stamp for every markdown file [`Vault::notes`] would read, sorted.
    fn stamps(&self) -> Result<Vec<FileStamp>, VaultError> {
        let mut stamps = Vec::new();
        collect_markdown(&self.root, &self.root, &mut stamps)?;
        stamps.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(stamps)
    }

    /// Notes matching every word of `query`, best first.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, VaultError> {
        Ok(self.index()?.search(query, limit)?)
    }

    /// The vault-relative path of the note named by `key`: a path, an id, or a
    /// slug that only one note has.
    pub fn find(&self, key: &str) -> Result<String, VaultError> {
        if key.contains('/') || key.ends_with(".md") {
            let rel =
                RelPath::parse(key).map_err(|e| VaultError::BadPath(key.into(), e.to_string()))?;
            return if self.root.join(rel.as_str()).is_file() {
                Ok(rel.as_str().to_string())
            } else {
                Err(VaultError::NotFound(key.into()))
            };
        }
        let mut paths = self.index()?.lookup(key)?;
        match paths.len() {
            0 => Err(VaultError::NotFound(key.into())),
            1 => Ok(paths.remove(0)),
            _ => Err(VaultError::Ambiguous(key.into(), paths.join(", "))),
        }
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
        self.stamps()?
            .into_iter()
            .map(|stamp| {
                let note = self.parse_file(&stamp.path)?;
                Ok(VaultFile {
                    rel: stamp.path,
                    note,
                })
            })
            .collect()
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

/// Refuse `id` when a file other than `rel` already carries it.
fn check_id_free(index: &Index, id: &NoteId, rel: &str) -> Result<(), VaultError> {
    match index
        .paths_with_id(id.as_str())?
        .into_iter()
        .find(|p| p != rel)
    {
        Some(existing) => Err(VaultError::DuplicateId {
            id: id.as_str().to_string(),
            existing,
        }),
        None => Ok(()),
    }
}

fn collect_markdown(dir: &Path, root: &Path, out: &mut Vec<FileStamp>) -> Result<(), VaultError> {
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
                let meta = entry
                    .metadata()
                    .map_err(io_error(format!("stat {}", path.display())))?;
                let mtime_ns = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
                out.push(FileStamp {
                    path: rel.to_string_lossy().replace('\\', "/"),
                    size: meta.len(),
                    mtime_ns,
                });
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
