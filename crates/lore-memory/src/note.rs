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

/// A repository's name for people: the last path segment of its origin URL,
/// without `.git`. Names the repo's vault folder next to its [`RepoKey`]; the
/// key stays the identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoName(String);

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
/// current statement first, then an append-only `## Timeline`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: NoteId,
    pub note_type: NoteType,
    pub title: String,
    pub scope: Scope,
    /// The repository's display name, written `repo:`. Names the folder.
    pub repo: Option<RepoName>,
    pub status: Status,
    pub anchors: Vec<Anchor>,
    pub relations: Vec<Relation>,
    pub sources: Vec<SourceRef>,
    pub created: NoteDate,
    pub forget_after: Option<NoteDate>,
    pub body: String,
}

/// A value that does not fit its type. The text says what was expected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct InvalidValue(String);

fn invalid(problem: impl Into<String>) -> InvalidValue {
    InvalidValue(problem.into())
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

impl NoteId {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        let ok = s.len() == 26
            && s.bytes().all(|b| CROCKFORD.contains(&b))
            && s.as_bytes().first().is_some_and(|b| *b <= b'7');
        if ok {
            Ok(Self(s.to_string()))
        } else {
            Err(invalid(format!(
                "expected a 26-character uppercase ULID, got `{s}`"
            )))
        }
    }

    /// A new id: the current millisecond, then 80 bits that differ per call.
    #[must_use]
    pub fn generate() -> Self {
        use std::hash::{BuildHasher, Hasher};
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let millis = now.as_millis() & ((1 << 48) - 1);
        let mut first = std::collections::hash_map::RandomState::new().build_hasher();
        first.write_u128(now.as_nanos());
        first.write_u32(std::process::id());
        let high = first.finish();
        let mut second = std::collections::hash_map::RandomState::new().build_hasher();
        second.write_u64(high);
        let low = second.finish() & 0xFFFF;
        let value = (millis << 80) | (u128::from(high) << 16) | u128::from(low);
        let text = (0..26)
            .map(|i| {
                let digit = (value >> (125 - 5 * i)) & 31;
                char::from(CROCKFORD[digit as usize])
            })
            .collect();
        Self(text)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl NoteType {
    pub const ALL: [NoteType; 5] = [
        NoteType::Convention,
        NoteType::Decision,
        NoteType::Fact,
        NoteType::Howto,
        NoteType::Pitfall,
    ];

    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        Self::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| {
                invalid(format!(
                    "expected convention, decision, fact, howto, or pitfall, got `{s}`"
                ))
            })
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            NoteType::Convention => "convention",
            NoteType::Decision => "decision",
            NoteType::Fact => "fact",
            NoteType::Howto => "howto",
            NoteType::Pitfall => "pitfall",
        }
    }
}

impl RepoKey {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        if (8..=64).contains(&s.len()) && is_lower_hex(s) {
            Ok(Self(s.to_string()))
        } else {
            Err(invalid(format!(
                "expected 8 to 64 lowercase hex characters, got `{s}`"
            )))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl RepoName {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        let ok = !s.trim().is_empty()
            && s.trim() == s
            && s.len() <= 200
            && !s.chars().any(char::is_control);
        if ok {
            Ok(Self(s.to_string()))
        } else {
            Err(invalid(format!(
                "expected a repository name on one line, got `{s}`"
            )))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Scope {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        if s == "global" {
            return Ok(Scope::Global);
        }
        match s.strip_prefix("repo:") {
            Some(key) => Ok(Scope::Repo(RepoKey::parse(key)?)),
            None => Err(invalid(format!(
                "expected `global` or `repo:<hex>`, got `{s}`"
            ))),
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::Global => f.write_str("global"),
            Scope::Repo(key) => write!(f, "repo:{}", key.as_str()),
        }
    }
}

impl Status {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        match s {
            "active" => Ok(Status::Active),
            "retired" => Ok(Status::Retired),
            _ => Err(invalid(format!("expected active or retired, got `{s}`"))),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Retired => "retired",
        }
    }
}

impl RelPath {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        if s.is_empty() {
            return Err(invalid("the path is empty"));
        }
        if s.starts_with('/') || s.as_bytes().get(1) == Some(&b':') {
            return Err(invalid(format!(
                "absolute paths are not allowed, got `{s}`"
            )));
        }
        if s.contains('\\') || s.chars().any(char::is_control) {
            return Err(invalid(format!(
                "use `/` separators and no control characters, got `{s}`"
            )));
        }
        for part in s.split('/') {
            match part {
                ".." => return Err(invalid(format!("`..` is not allowed, got `{s}`"))),
                "" | "." => {
                    return Err(invalid(format!(
                        "empty and `.` path segments are not allowed, got `{s}`"
                    )))
                }
                _ => {}
            }
        }
        Ok(Self(s.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl LineRange {
    pub fn new(start: u32, end: u32) -> Result<Self, InvalidValue> {
        if start == 0 || end < start {
            return Err(invalid(format!(
                "expected 1-based lines with start <= end, got {start}-{end}"
            )));
        }
        Ok(Self { start, end })
    }

    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        let number = |n: &str| {
            n.parse::<u32>()
                .ok()
                .filter(|_| n.bytes().all(|b| b.is_ascii_digit()))
                .ok_or_else(|| invalid(format!("expected lines like `12-20`, got `{s}`")))
        };
        match s.split_once('-') {
            Some((a, b)) => Self::new(number(a)?, number(b)?),
            None => {
                let line = number(s)?;
                Self::new(line, line)
            }
        }
    }

    #[must_use]
    pub fn start(self) -> u32 {
        self.start
    }

    #[must_use]
    pub fn end(self) -> u32 {
        self.end
    }
}

impl fmt::Display for LineRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start == self.end {
            write!(f, "{}", self.start)
        } else {
            write!(f, "{}-{}", self.start, self.end)
        }
    }
}

impl LineHash {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        match s.strip_prefix("b3:") {
            Some(hex) if (8..=64).contains(&hex.len()) && is_lower_hex(hex) => {
                Ok(Self(s.to_string()))
            }
            _ => Err(invalid(format!(
                "expected `b3:` and 8 to 64 lowercase hex characters, got `{s}`"
            ))),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CommitId {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        if (7..=64).contains(&s.len()) && is_lower_hex(s) {
            Ok(Self(s.to_string()))
        } else {
            Err(invalid(format!(
                "expected 7 to 64 lowercase hex characters, got `{s}`"
            )))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Relation {
    pub fn parse(kind: &str, id: &str) -> Result<Self, InvalidValue> {
        let id = NoteId::parse(id)?;
        match kind {
            "updates" => Ok(Relation::Updates(id)),
            "extends" => Ok(Relation::Extends(id)),
            "derives" => Ok(Relation::Derives(id)),
            _ => Err(invalid(format!(
                "expected updates, extends, or derives, got `{kind}`"
            ))),
        }
    }

    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Relation::Updates(_) => "updates",
            Relation::Extends(_) => "extends",
            Relation::Derives(_) => "derives",
        }
    }

    #[must_use]
    pub fn target(&self) -> &NoteId {
        match self {
            Relation::Updates(id) | Relation::Extends(id) | Relation::Derives(id) => id,
        }
    }
}

impl AgentKind {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        match s {
            "claude-code" => Ok(AgentKind::ClaudeCode),
            "codex" => Ok(AgentKind::Codex),
            _ => Err(invalid(format!("expected claude-code or codex, got `{s}`"))),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Codex => "codex",
        }
    }
}

impl SessionId {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        let ok = (1..=200).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if ok {
            Ok(Self(s.to_string()))
        } else {
            Err(invalid(format!(
                "expected a session id of letters, digits, `.`, `_`, `-`, got `{s}`"
            )))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl SourceRef {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        let (agent, session) = s
            .split_once(':')
            .ok_or_else(|| invalid(format!("expected `agent:session`, got `{s}`")))?;
        Ok(Self {
            agent: AgentKind::parse(agent)?,
            session: SessionId::parse(session)?,
        })
    }
}

impl fmt::Display for SourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.agent.as_str(), self.session.as_str())
    }
}

impl NoteDate {
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        let bad = || invalid(format!("expected a date like 2026-11-01, got `{s}`"));
        let bytes = s.as_bytes();
        let shaped = bytes.len() == 10
            && bytes.get(4) == Some(&b'-')
            && bytes.get(7) == Some(&b'-')
            && bytes
                .iter()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
        if !shaped {
            return Err(bad());
        }
        let part = |range: std::ops::Range<usize>| {
            s.get(range)
                .and_then(|p| p.parse::<u16>().ok())
                .ok_or_else(bad)
        };
        let year = i32::from(part(0..4)?);
        let month = u8::try_from(part(5..7)?).map_err(|_| bad())?;
        let day = u8::try_from(part(8..10)?).map_err(|_| bad())?;
        let month = time::Month::try_from(month).map_err(|_| bad())?;
        time::Date::from_calendar_date(year, month, day)
            .map(Self)
            .map_err(|_| bad())
    }

    /// Today's date in UTC.
    #[must_use]
    pub fn today() -> Self {
        Self(time::OffsetDateTime::now_utc().date())
    }
}

impl fmt::Display for NoteDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}",
            self.0.year(),
            u8::from(self.0.month()),
            self.0.day()
        )
    }
}
