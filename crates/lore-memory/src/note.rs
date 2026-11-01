//! The note: its types, and how it is read from and written to markdown.
//!
//! Every field Lore understands has a type that can only hold a valid value, so
//! a bad path, id, or date is refused when the file is parsed rather than
//! discovered later by whatever reads it.

use std::fmt;

use thiserror::Error;

/// A note's stable id: a ULID, 26 characters of Crockford base32.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteId(String);

/// What kind of knowledge a note holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteType {
    Convention,
    Decision,
    Fact,
    Howto,
    Pitfall,
}

/// A repository's identity: a lowercase hex hash of its normalized origin URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoKey(String);

/// Where a note applies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    Repo(RepoKey),
}

/// Whether a note is current.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Active,
    Retired,
}

/// A repository-relative path with `/` separators. Never absolute, never `..`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelPath(String);

/// An inclusive, 1-based line range. Written `210-224`, or `7` for one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineRange {
    start: u32,
    end: u32,
}

/// A digest of the anchored lines, written `b3:<hex>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LineHash(String);

/// An abbreviated or full git commit id, lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId(String);

/// A place in a repository that a note is about. M2 checks it for freshness.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub path: RelPath,
    pub lines: Option<LineRange>,
    pub hash: Option<LineHash>,
    pub commit: Option<CommitId>,
}

/// How a note relates to an earlier one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Relation {
    Updates(NoteId),
    Extends(NoteId),
    Derives(NoteId),
}

/// The agent that recorded an archived session, named as the archive names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
}

/// An archived session id, the same string `read_session` takes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

/// Provenance: the archived session a note came from. Written `agent:session`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceRef {
    pub agent: AgentKind,
    pub session: SessionId,
}

/// A calendar date, written `YYYY-MM-DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteDate(time::Date);

/// A note.
///
/// `body` is everything after the closing `---` fence, kept byte for byte: the
/// current statement first, then an append-only `## Timeline`. `extra` keeps
/// the frontmatter's layout and every key Lore does not own, so a parse then a
/// render never loses an Obsidian plugin's keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: NoteId,
    pub note_type: NoteType,
    pub title: String,
    pub scope: Scope,
    pub status: Status,
    pub anchors: Vec<Anchor>,
    pub relations: Vec<Relation>,
    pub sources: Vec<SourceRef>,
    pub created: NoteDate,
    pub forget_after: Option<NoteDate>,
    pub body: String,
    pub extra: Frontmatter,
}

/// The frontmatter's layout: Lore's keys in the order the file had them, with
/// unknown keys, comments, and blank lines kept as verbatim text in place.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Frontmatter {
    slots: Vec<Slot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Slot {
    Known(Key),
    Raw(String),
}

/// The frontmatter keys Lore owns, in the order a new note writes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Key {
    Id,
    Type,
    Title,
    Scope,
    Status,
    Anchors,
    Relations,
    Sources,
    Created,
    ForgetAfter,
}

/// Why a file is not a valid note. Every variant names what to fix.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NoteError {
    #[error("not a note: {0}")]
    NotANote(&'static str),
    #[error("missing required key `{0}`")]
    Missing(&'static str),
    #[error("key `{0}` appears twice")]
    Duplicate(&'static str),
    #[error("`{field}`: {problem}")]
    Field { field: String, problem: String },
}
