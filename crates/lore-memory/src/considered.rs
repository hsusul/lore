//! Which mined corrections a lessons run has already read.
//!
//! Every correction the miner finds ([`mine::Candidate`]) has a
//! [`CorrectionId`] derived from its archive session, its timestamp, and its
//! text, so the same correction keeps its id across runs, re-scans, and
//! however [`mine::lessons`] groups it. Lesson ids depend on grouping and are
//! never used here.
//!
//! The ledger is `<vault>/.lore/considered.jsonl`, gitignored with the rest
//! of `.lore/`: one JSON line `{id, considered_at, run}` per correction a
//! run read, appended with one `O_APPEND` write per line, like proposals. It
//! is a set: reading dedupes and ignores a line torn by a crash, so recording
//! an id twice is harmless. Only a run that finished records anything, so a
//! failed run's corrections are read again by the next.
//!
//! It sits in `.lore/`, not `_inbox/`, because older Lore reads every
//! `_inbox/*.jsonl` as proposals. Losing it is safe: the next run reads
//! every correction again.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::mine::{self, Candidate, Lesson};
use crate::vault::{Vault, VaultError, LORE_DIR};

/// The ledger's file name inside the vault's [`LORE_DIR`].
pub const CONSIDERED_FILE: &str = "considered.jsonl";
/// What every [`CorrectionId`] starts with.
const PREFIX: &str = "cr-";
/// Hex characters after the prefix: 64 bits.
const HEX_CHARS: usize = 16;

/// The stable id of one mined correction, written `cr-<16 hex>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CorrectionId(String);

impl CorrectionId {
    /// The id of `candidate`: BLAKE3 over its session, timestamp, and text.
    #[must_use]
    pub fn of(candidate: &Candidate) -> Self {
        let at = candidate
            .at_ms
            .map_or_else(|| "-".to_owned(), |ms| ms.to_string());
        let hash =
            blake3::hash(format!("{}\n{at}\n{}", candidate.session, candidate.text).as_bytes());
        Self(format!("{PREFIX}{}", &hash.to_hex()[..HEX_CHARS]))
    }

    /// `text` as an id, when it is one: `cr-` and 16 lowercase hex characters.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix(PREFIX)?;
        (hex.len() == HEX_CHARS && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CorrectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Every id in the text of a file listing them, one per line or one JSON
/// object with an `id` per line. Blank, torn, and foreign lines are ignored.
#[must_use]
pub fn parse_ids(bytes: &[u8]) -> BTreeSet<CorrectionId> {
    bytes
        .split(|b| *b == b'\n')
        .filter_map(|raw| std::str::from_utf8(raw).ok())
        .map(str::trim)
        .filter_map(|line| {
            CorrectionId::parse(line).or_else(|| {
                let value: Value = serde_json::from_str(line).ok()?;
                CorrectionId::parse(value.get("id")?.as_str()?)
            })
        })
        .collect()
}

/// The ids in the file at `path`; none when it does not exist.
pub fn read_ids(path: &Path) -> io::Result<BTreeSet<CorrectionId>> {
    match fs::read(path) {
        Ok(bytes) => Ok(parse_ids(&bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(e),
    }
}

/// Every correction a finished run has read, from the vault's ledger.
pub fn considered(vault: &Vault) -> Result<BTreeSet<CorrectionId>, VaultError> {
    let path = vault.lore_dir().join(CONSIDERED_FILE);
    read_ids(&path).map_err(|e| VaultError::Io(format!("read {LORE_DIR}/{CONSIDERED_FILE}"), e))
}

/// Record that run `run` read `ids`: one appended line each. Recording an id
/// already in the ledger adds a line that reading folds away.
pub fn record<'a>(
    vault: &Vault,
    ids: impl IntoIterator<Item = &'a CorrectionId>,
    run: &str,
) -> Result<(), VaultError> {
    let io = |what: &str| {
        let what = format!("{what} {LORE_DIR}/{CONSIDERED_FILE}");
        move |e| VaultError::Io(what, e)
    };
    let dir = vault.lore_dir();
    fs::create_dir_all(&dir).map_err(|e| VaultError::Io(format!("create {LORE_DIR}"), e))?;
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(dir.join(CONSIDERED_FILE))
        .map_err(io("open"))?;
    // Start a fresh line after one torn by a crash, so the first id is not
    // glued onto it and lost with it.
    let mut torn = crate::review::ends_torn(&mut file).map_err(io("read"))?;
    let at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    for id in ids {
        let mut line = if torn { "\n".to_owned() } else { String::new() };
        torn = false;
        line.push_str(&json!({ "id": id.as_str(), "considered_at": at, "run": run }).to_string());
        line.push('\n');
        file.write_all(line.as_bytes()).map_err(io("append to"))?;
    }
    Ok(())
}

/// A lesson with each receipt's id and whether it is new: not yet read by a
/// finished run, or, for a pinned run, one of the run's corrections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marked {
    lesson: Lesson,
    /// One per receipt, in receipt order.
    marks: Vec<(CorrectionId, bool)>,
}

impl Marked {
    /// `lesson` with each receipt marked new when `is_new` says so.
    #[must_use]
    pub fn new(lesson: Lesson, is_new: impl Fn(&CorrectionId) -> bool) -> Self {
        let marks = (lesson.receipts.iter())
            .map(|r| {
                let id = CorrectionId::of(r);
                let new = is_new(&id);
                (id, new)
            })
            .collect();
        Self { lesson, marks }
    }

    #[must_use]
    pub fn lesson(&self) -> &Lesson {
        &self.lesson
    }

    /// Each receipt, newest first, with its id and whether it is new.
    pub fn receipts(&self) -> impl Iterator<Item = (&Candidate, &CorrectionId, bool)> {
        (self.lesson.receipts.iter())
            .zip(&self.marks)
            .map(|(r, (id, new))| (r, id, *new))
    }

    /// Whether any receipt is new.
    #[must_use]
    pub fn has_new(&self) -> bool {
        self.marks.iter().any(|(_, new)| *new)
    }

    /// The ids of the new receipts.
    pub fn new_ids(&self) -> impl Iterator<Item = &CorrectionId> {
        self.marks.iter().filter(|(_, new)| *new).map(|(id, _)| id)
    }
}

/// `lessons`, each marked, keeping only those with a new receipt unless
/// `all_seen`. Older receipts stay in a kept lesson as context.
#[must_use]
pub fn mark(
    lessons: Vec<Lesson>,
    is_new: impl Fn(&CorrectionId) -> bool,
    all_seen: bool,
) -> Vec<Marked> {
    lessons
        .into_iter()
        .map(|l| Marked::new(l, &is_new))
        .filter(|m| all_seen || m.has_new())
        .collect()
}

/// The corrections a lessons run would read: the new receipts of the lessons
/// `mine_candidates` lists by default (default limits, one-off orders left
/// out), given what earlier runs `considered`.
#[must_use]
pub fn unconsidered(
    turns: &mine::Turns,
    considered: &BTreeSet<CorrectionId>,
) -> BTreeSet<CorrectionId> {
    let mut lessons = mine::lessons(mine::corrections_in(turns, &mine::Limits::default()));
    lessons.retain(|l| l.score >= 0);
    mark(lessons, |id| !considered.contains(id), false)
        .iter()
        .flat_map(|m| m.new_ids().cloned().collect::<Vec<_>>())
        .collect()
}
