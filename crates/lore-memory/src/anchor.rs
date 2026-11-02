//! Whether the code a note is about still looks the way it did.
//!
//! An anchor names a path, optionally a line range, and optionally a hash of
//! those lines. The hash is BLAKE3 over each anchored line with a trailing CR
//! removed and a LF appended, written `b3:<hex>`; a stored hash may be any
//! prefix of 8 to 64 hex characters. The same digest is
//! `sed -n '<a>,<b>p' FILE | tr -d '\r' | b3sum`.
//!
//! Checks read the working tree, never write it, and never follow a path out
//! of the repository.

use std::cell::OnceCell;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::note::{Anchor, CommitId, LineHash, LineRange, RelPath};
use crate::repo::git;

/// Hex characters kept when Lore writes a line hash.
const PINNED_HASH_LEN: usize = 16;

/// How an anchor compares with the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Freshness {
    /// The anchored lines are where the note says.
    Fresh,
    /// The file is there but the anchored lines are not.
    Changed,
    /// The file is not there.
    Gone,
}

impl Freshness {
    /// The word printed beside a recalled note.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Freshness::Fresh => "fresh",
            Freshness::Changed => "changed",
            Freshness::Gone => "gone",
        }
    }
}

/// Why an anchor cannot be pinned to the working tree.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PinError {
    #[error("anchor `{0}` is not a file in the repository")]
    Missing(String),
    #[error("anchor `{path}` has {len} lines, so lines {lines} do not exist")]
    OutOfRange {
        path: String,
        lines: LineRange,
        len: usize,
    },
}

impl Anchor {
    /// Check this anchor against the working tree at `repo`.
    #[must_use]
    pub fn check(&self, repo: &Path) -> Freshness {
        WorkTree::new(repo)
            .check_at(self, &self.path)
            .unwrap_or(Freshness::Gone)
    }
}

/// A working tree that anchors are checked against.
#[derive(Debug)]
pub struct WorkTree {
    root: PathBuf,
    canonical_root: OnceCell<Option<PathBuf>>,
    head: OnceCell<Option<CommitId>>,
}

impl WorkTree {
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            canonical_root: OnceCell::new(),
            head: OnceCell::new(),
        }
    }

    /// Fill in a hash for an anchor with lines and none, and the commit from
    /// `HEAD` when the anchor names none. Fails when the file or lines are not
    /// in the working tree.
    pub fn pin(&self, anchor: &Anchor) -> Result<Anchor, PinError> {
        let missing = || PinError::Missing(anchor.path.as_str().to_string());
        let file = self.file(&anchor.path).ok_or_else(missing)?;
        let mut pinned = anchor.clone();
        if let (Some(lines), None) = (anchor.lines, &anchor.hash) {
            let digest = range_digest(&file, lines)
                .map_err(|_| missing())?
                .ok_or_else(|| PinError::OutOfRange {
                    path: anchor.path.as_str().to_string(),
                    lines,
                    len: fs::read(&file).map(|b| split_lines(&b).len()).unwrap_or(0),
                })?;
            pinned.hash = LineHash::parse(&format!("b3:{}", &digest[..PINNED_HASH_LEN])).ok();
        }
        if pinned.commit.is_none() {
            pinned.commit = self.head().cloned();
        }
        Ok(pinned)
    }

    fn head(&self) -> Option<&CommitId> {
        self.head
            .get_or_init(|| {
                git(&self.root, ["rev-parse", "--short=12", "HEAD"])
                    .and_then(|out| CommitId::parse(out.trim()).ok())
            })
            .as_ref()
    }

    /// The anchor checked at `path`. `None` when there is no such file.
    fn check_at(&self, anchor: &Anchor, path: &RelPath) -> Option<Freshness> {
        let file = self.file(path)?;
        let Some(lines) = anchor.lines else {
            return Some(Freshness::Fresh);
        };
        let Ok(digest) = range_digest(&file, lines) else {
            return Some(Freshness::Changed);
        };
        let Some(hash) = &anchor.hash else {
            return Some(if digest.is_some() {
                Freshness::Fresh
            } else {
                Freshness::Changed
            });
        };
        if digest.is_some_and(|d| hash_matches(hash, &d)) {
            return Some(Freshness::Fresh);
        }
        Some(Freshness::Changed)
    }

    /// The file at `path`, when it is a regular file inside the working tree.
    fn file(&self, path: &RelPath) -> Option<PathBuf> {
        let candidate = self.root.join(path.as_str());
        let resolved = fs::canonicalize(&candidate).ok()?;
        let root = self
            .canonical_root
            .get_or_init(|| fs::canonicalize(&self.root).ok())
            .as_ref()?;
        (resolved.starts_with(root) && resolved.is_file()).then_some(resolved)
    }
}

/// The line hash of `lines`, in full hex.
#[must_use]
pub fn hash_lines<'a>(lines: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut hasher = blake3::Hasher::new();
    for line in lines {
        hasher.update(line.strip_suffix(b"\r").unwrap_or(line));
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_matches(stored: &LineHash, digest: &str) -> bool {
    stored
        .as_str()
        .strip_prefix("b3:")
        .is_some_and(|hex| digest.starts_with(hex))
}

/// The digest of `lines` in `file`, read only as far as the last of them.
/// `None` when the file is shorter than the range.
fn range_digest(file: &Path, lines: LineRange) -> io::Result<Option<String>> {
    let mut reader = BufReader::new(File::open(file)?);
    let mut taken = Vec::new();
    let mut line = Vec::new();
    for number in 1..=lines.end() {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(None);
        }
        if number >= lines.start() {
            taken.push(line.strip_suffix(b"\n").unwrap_or(&line).to_vec());
        }
    }
    Ok(Some(hash_lines(taken.iter().map(Vec::as_slice))))
}

fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if body.is_empty() && bytes.is_empty() {
        return Vec::new();
    }
    body.split(|b| *b == b'\n').collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_and_lf_lines_hash_the_same() {
        let lf = hash_lines([&b"a"[..], b"b"]);
        let crlf = hash_lines([&b"a\r"[..], b"b\r"]);
        assert_eq!(lf, crlf);
        assert_eq!(
            lf,
            blake3::hash(b"a\nb\n").to_hex().to_string(),
            "the digest is plain BLAKE3 of the LF-terminated lines"
        );
    }

    #[test]
    fn split_lines_ignores_one_trailing_newline() {
        assert_eq!(split_lines(b""), Vec::<&[u8]>::new());
        assert_eq!(split_lines(b"a\nb\n"), vec![&b"a"[..], b"b"]);
        assert_eq!(split_lines(b"a\nb"), vec![&b"a"[..], b"b"]);
        assert_eq!(split_lines(b"\n"), vec![&b""[..]]);
    }
}
