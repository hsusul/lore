//! Filing a mined [`Lesson`] into the vault's inbox, the same path as every
//! agent proposal, so a person accepts or rejects it.
//!
//! A lesson is proposed as the agent of its newest receipt, under that
//! receipt's archive session id, marked verified: the miner read that session
//! from the archive, and it is where the person said it. Its body carries a
//! marker with the lesson id, so one lesson is filed once. A lesson from one
//! repository is anchored to the files of it that its receipts name.

use std::path::Path;

use time::OffsetDateTime;

use crate::anchor::WorkTree;
use crate::inbox::{self, Proposal};
use crate::mine::{Candidate, Lesson};
use crate::note::{AgentKind, Anchor, InvalidValue, NoteId, NoteType, RelPath, Scope, SessionId};
use crate::repo::RepoContext;
use crate::review::{self, ReviewError};
use crate::vault::Vault;

/// Receipts written into a proposed lesson's body.
const PROPOSED_RECEIPTS: usize = 10;
/// The longest title a proposal gets, as over MCP.
pub const TITLE_CHARS: usize = 120;

/// Why a lesson could not be filed.
#[derive(Debug, thiserror::Error)]
pub enum FilingError {
    /// The newest receipt's agent or session is not a valid note value.
    #[error(transparent)]
    Invalid(#[from] InvalidValue),
    #[error(transparent)]
    Review(#[from] ReviewError),
}

/// The tag a lesson's proposal body carries, so one lesson is filed once.
fn marker(lesson: &Lesson) -> String {
    format!("(lesson {})", lesson.id)
}

/// True when one of the `filed` proposals came from `lesson`, whatever was
/// decided about it since.
#[must_use]
pub fn is_filed(filed: &[Proposal], lesson: &Lesson) -> bool {
    filed.iter().any(|p| came_from(p, lesson))
}

/// Whether `proposal` was filed from `lesson`.
#[must_use]
pub fn came_from(proposal: &Proposal, lesson: &Lesson) -> bool {
    proposal.body.contains(&marker(lesson))
}

/// File `lesson` with `title`, then accept it: one commit, and the decision
/// recorded for eval. Returns the note's vault path.
pub fn keep(vault: &Vault, lesson: &Lesson, title: &str) -> Result<String, FilingError> {
    let mut proposal = proposal(lesson)?;
    proposal.title = title.chars().take(TITLE_CHARS).collect();
    let written = inbox::propose(vault, proposal).map_err(ReviewError::from)?;
    Ok(review::accept(vault, written.id.as_str(), None)?.rel)
}

/// The proposal that files `lesson`: its rule as the title and first line,
/// then up to [`PROPOSED_RECEIPTS`] receipts.
pub fn proposal(lesson: &Lesson) -> Result<Proposal, InvalidValue> {
    let marker = marker(lesson);
    let newest = &lesson.receipts[0];
    let mut body = format!(
        "{}\n\nMined by `lorectl mem mine` {marker} from corrections in {} conversation{}:\n",
        lesson.text,
        lesson.conversations,
        if lesson.conversations == 1 { "" } else { "s" }
    );
    for receipt in lesson.receipts.iter().take(PROPOSED_RECEIPTS) {
        body.push_str(&format!(
            "- {} {} {}: \"{}\"",
            day(receipt),
            receipt.agent,
            receipt.session,
            receipt.text
        ));
        if let Some(after) = &receipt.preceding {
            body.push_str(&format!(", after \"{after}\""));
        }
        body.push('\n');
    }
    let more = lesson.receipts.len().saturating_sub(PROPOSED_RECEIPTS);
    if more > 0 {
        body.push_str(&format!("- and {more} more sessions\n"));
    }
    let repo = repo(lesson);
    let scope = repo
        .key()
        .map_or(Scope::Global, |key| Scope::Repo(key.clone()));
    let anchors = match (&scope, repo.root()) {
        (Scope::Repo(_), Some(root)) => anchors(lesson, root),
        _ => Vec::new(),
    };
    Ok(Proposal {
        id: NoteId::generate(),
        agent: AgentKind::parse(&newest.agent)?,
        session: SessionId::parse(&newest.session)?,
        source_verified: true,
        note_type: NoteType::Convention,
        scope,
        title: lesson.text.chars().take(TITLE_CHARS).collect(),
        body,
        anchors,
        relation: None,
        enforce: Vec::new(),
        proposed_at: OffsetDateTime::now_utc(),
    })
}

/// A receipt's UTC `YYYY-MM-DD`, or "undated".
fn day(candidate: &Candidate) -> String {
    candidate
        .at_ms
        .and_then(|ms| OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000)).ok())
        .map_or_else(|| "undated".into(), |at| at.date().to_string())
}

/// The repository every receipt came from, when they share one this machine
/// can still find; otherwise none, and the lesson is global.
fn repo(lesson: &Lesson) -> RepoContext {
    let mut paths = lesson.receipts.iter().map(|r| r.repo_path.as_deref());
    let first = paths.next().flatten();
    match first.filter(|first| paths.all(|p| p == Some(first))) {
        Some(path) => RepoContext::discover(Path::new(path)),
        None => RepoContext::NoRepo,
    }
}

/// A whole-file anchor, pinned to `HEAD`, for each file in the working tree at
/// `root` that a proposed receipt names: in the person's words, or in the
/// agent turns before and after them. A name that is not a file there is
/// left out.
fn anchors(lesson: &Lesson, root: &Path) -> Vec<Anchor> {
    let tree = WorkTree::new(root);
    let texts = lesson
        .receipts
        .iter()
        .take(PROPOSED_RECEIPTS)
        .flat_map(|r| [Some(&r.text), r.preceding.as_ref(), r.following.as_ref()])
        .flatten();
    let words = texts
        .flat_map(|text| text.split(|c: char| c.is_whitespace() || "`'\"()[]{}<>,;".contains(c)));
    let mut anchors: Vec<Anchor> = Vec::new();
    for path in words.filter_map(|word| named_path(word, root)) {
        if anchors.iter().any(|a| a.path == path) {
            continue;
        }
        let anchor = Anchor {
            path,
            lines: None,
            hash: None,
            commit: None,
        };
        if let Ok(pinned) = tree.pin(&anchor) {
            anchors.push(pinned);
        }
    }
    anchors
}

/// `word` as a path inside `root`, when it reads like one: it has a `/` or a
/// `.`. Trailing sentence punctuation and a line number such as `:12` are
/// dropped, and an absolute path under `root` is made relative.
fn named_path(word: &str, root: &Path) -> Option<RelPath> {
    let word = word.trim_end_matches(['.', ':', '!', '?']);
    let word = match word.split_once(':') {
        Some((path, at)) if at.bytes().all(|b| b.is_ascii_digit() || b"-:".contains(&b)) => path,
        _ => word,
    };
    let word = match Path::new(word).strip_prefix(root) {
        Ok(inside) => inside.to_str()?,
        Err(_) => word,
    };
    let word = word.strip_prefix("./").unwrap_or(word);
    if !word.contains(['/', '.']) {
        return None;
    }
    RelPath::parse(word).ok()
}
