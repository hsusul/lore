//! Filing a mined [`Lesson`] into the vault's inbox, the same path as every
//! agent proposal, so a person accepts or rejects it.
//!
//! A lesson is proposed as the agent of its newest receipt, under that
//! receipt's archive session id, marked verified: the miner read that session
//! from the archive, and it is where the person said it. Its body carries a
//! marker with the lesson id, so one lesson is filed once.

use std::path::Path;

use time::OffsetDateTime;

use crate::inbox::{self, Proposal};
use crate::mine::{Candidate, Lesson};
use crate::note::{AgentKind, InvalidValue, NoteId, NoteType, Scope, SessionId};
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
    Ok(Proposal {
        id: NoteId::generate(),
        agent: AgentKind::parse(&newest.agent)?,
        session: SessionId::parse(&newest.session)?,
        source_verified: true,
        note_type: NoteType::Convention,
        scope: scope(lesson),
        title: lesson.text.chars().take(TITLE_CHARS).collect(),
        body,
        anchors: Vec::new(),
        relation: None,
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
/// can still find; otherwise global.
fn scope(lesson: &Lesson) -> Scope {
    let mut paths = lesson.receipts.iter().map(|r| r.repo_path.as_deref());
    let first = paths.next().flatten();
    match first.filter(|first| paths.all(|p| p == Some(first))) {
        Some(path) => RepoContext::discover(Path::new(path))
            .key()
            .map_or(Scope::Global, |key| Scope::Repo(key.clone())),
        None => Scope::Global,
    }
}
