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

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::note::{Anchor, CommitId, LineHash, LineRange, RelPath};
use crate::repo::git;

/// Files larger than this are not searched for anchored lines that moved
/// within them; a mismatch there reports [`Freshness::Changed`].
pub const MOVE_SEARCH_LIMIT: u64 = 1024 * 1024;

/// Hex characters kept when Lore writes a line hash.
const PINNED_HASH_LEN: usize = 16;

/// How an anchor, or a note, compares with the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Freshness {
    /// The anchored lines are where the note says.
    Fresh,
    /// The anchored lines are unchanged but now at `lines`, in the same file or
    /// in the file it was renamed to.
    Moved { lines: LineRange },
    /// The file is there but the anchored lines are not.
    Changed,
    /// The file is not there and no rename of it was found.
    Gone,
    /// The note has no anchors, so there is nothing to check.
    Unanchored,
}

impl Freshness {
    /// The word printed beside a recalled note.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Freshness::Fresh => "fresh",
            Freshness::Moved { .. } => "moved",
            Freshness::Changed => "changed",
            Freshness::Gone => "gone",
            Freshness::Unanchored => "unanchored",
        }
    }

    fn severity(self) -> u8 {
        match self {
            Freshness::Fresh | Freshness::Unanchored => 0,
            Freshness::Moved { .. } => 1,
            Freshness::Changed => 2,
            Freshness::Gone => 3,
        }
    }

    /// A note's freshness: its worst anchor, or `Unanchored` with none.
    #[must_use]
    pub fn of_note(anchors: &[Located]) -> Freshness {
        anchors
            .iter()
            .map(|located| located.freshness)
            .reduce(|worst, next| {
                if next.severity() > worst.severity() {
                    next
                } else {
                    worst
                }
            })
            .unwrap_or(Freshness::Unanchored)
    }
}

/// One anchor checked against a working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub freshness: Freshness,
    /// Where the anchored file is now: the anchor's own path, or the path it
    /// was renamed to.
    pub path: RelPath,
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
        WorkTree::new(repo).locate(self).freshness
    }
}

/// A working tree that anchors are checked against. Rename lookups are cached,
/// one `git diff` per anchor commit, so check many anchors with one `WorkTree`.
#[derive(Debug)]
pub struct WorkTree {
    root: PathBuf,
    canonical_root: OnceCell<Option<PathBuf>>,
    head: OnceCell<Option<CommitId>>,
    renames: RefCell<HashMap<String, HashMap<String, RelPath>>>,
}

impl WorkTree {
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            canonical_root: OnceCell::new(),
            head: OnceCell::new(),
            renames: RefCell::new(HashMap::new()),
        }
    }

    /// Where `anchor` is now, and how it compares.
    #[must_use]
    pub fn locate(&self, anchor: &Anchor) -> Located {
        if let Some(freshness) = self.check_at(anchor, &anchor.path) {
            return Located {
                freshness,
                path: anchor.path.clone(),
            };
        }
        let Some(new_path) = self.renamed(anchor) else {
            return Located {
                freshness: Freshness::Gone,
                path: anchor.path.clone(),
            };
        };
        let freshness = match self.check_at(anchor, &new_path) {
            Some(Freshness::Fresh) => match anchor.lines.or_else(|| self.whole_file(&new_path)) {
                Some(lines) => Freshness::Moved { lines },
                None => Freshness::Changed,
            },
            Some(other) => other,
            None => Freshness::Gone,
        };
        Located {
            freshness,
            path: new_path,
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
        let small = fs::metadata(&file).is_ok_and(|m| m.len() <= MOVE_SEARCH_LIMIT);
        let moved = small
            .then(|| fs::read(&file).ok())
            .flatten()
            .and_then(|bytes| find_lines(&bytes, lines.end() - lines.start() + 1, hash));
        Some(moved.map_or(Freshness::Changed, |lines| Freshness::Moved { lines }))
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

    fn whole_file(&self, path: &RelPath) -> Option<LineRange> {
        let bytes = fs::read(self.file(path)?).ok()?;
        let count = u32::try_from(split_lines(&bytes).len()).ok()?;
        LineRange::new(1, count.max(1)).ok()
    }

    /// The path `anchor`'s file was renamed to since the anchor's commit.
    fn renamed(&self, anchor: &Anchor) -> Option<RelPath> {
        let commit = anchor.commit.as_ref()?;
        let mut cache = self.renames.borrow_mut();
        let renames = cache
            .entry(commit.as_str().to_string())
            .or_insert_with(|| rename_map(&self.root, commit));
        renames.get(anchor.path.as_str()).cloned()
    }
}

/// Old path to new path for every rename between `commit` and the working
/// tree. Empty when git cannot tell, for example when the commit is unknown.
fn rename_map(root: &Path, commit: &CommitId) -> HashMap<String, RelPath> {
    let Some(out) = git(
        root,
        [
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames",
            "--diff-filter=R",
            "--name-status",
            "-z",
            commit.as_str(),
            "--",
        ],
    ) else {
        return HashMap::new();
    };
    let mut fields = out.split('\0');
    let mut renames = HashMap::new();
    while let Some(status) = fields.next() {
        if !status.starts_with('R') {
            continue;
        }
        if let (Some(old), Some(new)) = (fields.next(), fields.next()) {
            if let Ok(new) = RelPath::parse(new) {
                renames.insert(old.to_string(), new);
            }
        }
    }
    renames
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

/// The first run of `len` lines in `bytes` whose hash matches `hash`.
fn find_lines(bytes: &[u8], len: u32, hash: &LineHash) -> Option<LineRange> {
    let lines = split_lines(bytes);
    let len = usize::try_from(len).ok()?;
    if len == 0 || len > lines.len() {
        return None;
    }
    let start = (0..=lines.len() - len).find(|&start| {
        hash_matches(hash, &hash_lines(lines[start..start + len].iter().copied()))
    })?;
    let first = u32::try_from(start + 1).ok()?;
    let last = u32::try_from(start + len).ok()?;
    LineRange::new(first, last).ok()
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

    #[test]
    fn find_lines_reports_the_new_range() {
        let hash = LineHash::parse(&format!("b3:{}", &hash_lines([&b"x"[..], b"y"])[..8])).unwrap();
        assert_eq!(
            find_lines(b"a\nb\nx\ny\n", 2, &hash),
            Some(LineRange::new(3, 4).unwrap())
        );
        assert_eq!(find_lines(b"a\nb\n", 2, &hash), None);
    }

    #[test]
    fn worst_anchor_wins() {
        let at = |freshness| Located {
            freshness,
            path: RelPath::parse("a").unwrap(),
        };
        assert_eq!(Freshness::of_note(&[]), Freshness::Unanchored);
        let moved = Freshness::Moved {
            lines: LineRange::new(2, 3).unwrap(),
        };
        assert_eq!(
            Freshness::of_note(&[at(Freshness::Fresh), at(moved)]),
            moved
        );
        assert_eq!(
            Freshness::of_note(&[at(Freshness::Gone), at(Freshness::Changed)]),
            Freshness::Gone
        );
    }
}
