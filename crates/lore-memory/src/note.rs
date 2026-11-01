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

/// A value that does not fit its type. The text says what was expected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct InvalidValue(String);

fn invalid(problem: impl Into<String>) -> InvalidValue {
    InvalidValue(problem.into())
}

fn field_error(field: impl Into<String>, problem: impl fmt::Display) -> NoteError {
    NoteError::Field {
        field: field.into(),
        problem: problem.to_string(),
    }
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

    /// The vault directory that holds notes with this scope.
    #[must_use]
    pub fn dir(&self) -> String {
        match self {
            Scope::Global => "global".to_string(),
            Scope::Repo(key) => format!("repos/{}", key.as_str()),
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

impl Key {
    const ALL: [Key; 10] = [
        Key::Id,
        Key::Type,
        Key::Title,
        Key::Scope,
        Key::Status,
        Key::Anchors,
        Key::Relations,
        Key::Sources,
        Key::Created,
        Key::ForgetAfter,
    ];

    fn name(self) -> &'static str {
        match self {
            Key::Id => "id",
            Key::Type => "type",
            Key::Title => "title",
            Key::Scope => "scope",
            Key::Status => "status",
            Key::Anchors => "anchors",
            Key::Relations => "relations",
            Key::Sources => "sources",
            Key::Created => "created",
            Key::ForgetAfter => "forget_after",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

impl Frontmatter {
    fn has(&self, key: Key) -> bool {
        self.slots.contains(&Slot::Known(key))
    }

    /// Insert `key` where a new note would put it relative to the keys present.
    fn insert(&mut self, key: Key) {
        if self.has(key) {
            return;
        }
        let after = |slot: &Slot| matches!(slot, Slot::Known(k) if *k > key);
        let at = match self.slots.iter().position(after) {
            Some(at) => at,
            None => self
                .slots
                .iter()
                .rposition(|s| matches!(s, Slot::Known(_)))
                .map_or(0, |last| last + 1),
        };
        self.slots.insert(at, Slot::Known(key));
    }

    /// The names of the frontmatter keys Lore does not own, in file order.
    #[must_use]
    pub fn unknown_keys(&self) -> Vec<&str> {
        self.slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Raw(text) => {
                    let first = text.lines().next()?;
                    let starts_key = !first.starts_with([' ', '\t', '-', '#']);
                    let (key, _) = first.split_once(':')?;
                    starts_key.then_some(key.trim())
                }
                Slot::Known(_) => None,
            })
            .collect()
    }
}

/// The result of parsing a note someone is adding: the note, and which
/// required values were missing and filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub note: Note,
    pub id_filled: bool,
    pub created_filled: bool,
}

/// Values a draft takes when its file leaves them out.
struct Fill {
    id: NoteId,
    today: NoteDate,
}

impl Note {
    /// Parse a note file. Every required key must be present.
    pub fn parse(text: &str) -> Result<Note, NoteError> {
        parse_note(text, None).map(|draft| draft.note)
    }

    /// Parse a note someone is adding. `id`, `created`, `status`, and `scope`
    /// may be left out; they become `id`, `today`, `active`, and `global`.
    pub fn parse_draft(text: &str, id: NoteId, today: NoteDate) -> Result<Draft, NoteError> {
        parse_note(text, Some(Fill { id, today }))
    }

    /// Render the note as a file. `Note::parse(s)?.render() == s` for every
    /// file whose Lore-owned keys are in Lore's own formatting.
    #[must_use]
    pub fn render(&self) -> String {
        let mut layout = self.extra.clone();
        for key in Key::ALL {
            if self.has_value(key) {
                layout.insert(key);
            }
        }
        let mut out = String::from("---\n");
        for slot in &layout.slots {
            match slot {
                Slot::Known(key) => self.render_key(*key, &mut out),
                Slot::Raw(text) => out.push_str(text),
            }
        }
        out.push_str("---\n");
        out.push_str(&self.body);
        out
    }

    /// The file name stem: the title in lowercase ASCII words joined by `-`.
    #[must_use]
    pub fn slug(&self) -> String {
        let mut slug = String::new();
        for c in self.title.chars() {
            if c.is_ascii_alphanumeric() {
                if slug.len() >= 64 {
                    break;
                }
                slug.push(c.to_ascii_lowercase());
            } else if !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
        }
        let slug = slug.trim_end_matches('-');
        if slug.is_empty() {
            self.id.as_str().to_ascii_lowercase()
        } else {
            slug.to_string()
        }
    }

    /// Where the note lives in the vault: `<scope dir>/<slug>.md`.
    #[must_use]
    pub fn rel_path(&self) -> String {
        format!("{}/{}.md", self.scope.dir(), self.slug())
    }

    fn has_value(&self, key: Key) -> bool {
        match key {
            Key::Anchors => !self.anchors.is_empty(),
            Key::Relations => !self.relations.is_empty(),
            Key::Sources => !self.sources.is_empty(),
            Key::ForgetAfter => self.forget_after.is_some(),
            Key::Id | Key::Type | Key::Title | Key::Scope | Key::Status | Key::Created => true,
        }
    }

    fn render_key(&self, key: Key, out: &mut String) {
        let name = key.name();
        let line = |out: &mut String, value: &str| {
            out.push_str(name);
            out.push_str(": ");
            out.push_str(value);
            out.push('\n');
        };
        match key {
            Key::Id => line(out, self.id.as_str()),
            Key::Type => line(out, self.note_type.as_str()),
            Key::Title => line(out, &render_scalar(&self.title)),
            Key::Scope => line(out, &self.scope.to_string()),
            Key::Status => line(out, self.status.as_str()),
            Key::Created => line(out, &self.created.to_string()),
            Key::ForgetAfter => {
                if let Some(date) = self.forget_after {
                    line(out, &date.to_string());
                }
            }
            Key::Anchors | Key::Relations | Key::Sources => {
                let items: Vec<String> = match key {
                    Key::Anchors => self.anchors.iter().map(render_anchor).collect(),
                    Key::Relations => self
                        .relations
                        .iter()
                        .map(|r| format!("{}: {}", r.kind(), r.target().as_str()))
                        .collect(),
                    _ => self.sources.iter().map(ToString::to_string).collect(),
                };
                if items.is_empty() {
                    line(out, "[]");
                    return;
                }
                out.push_str(name);
                out.push_str(":\n");
                for item in items {
                    out.push_str("  - ");
                    out.push_str(&item);
                    out.push('\n');
                }
            }
        }
    }
}

fn render_anchor(anchor: &Anchor) -> String {
    let mut text = format!("path: {}", render_scalar(anchor.path.as_str()));
    if let Some(lines) = anchor.lines {
        text.push_str(&format!("\n    lines: {lines}"));
    }
    if let Some(hash) = &anchor.hash {
        text.push_str(&format!("\n    hash: {}", hash.as_str()));
    }
    if let Some(commit) = &anchor.commit {
        text.push_str(&format!("\n    commit: {}", commit.as_str()));
    }
    text
}

/// Plain when YAML would read the text back unchanged, double-quoted otherwise.
fn render_scalar(text: &str) -> String {
    const RESERVED: &[&str] = &[
        "true", "false", "yes", "no", "on", "off", "y", "n", "null", "~",
    ];
    let plain = !text.is_empty()
        && text.trim() == text
        && !text.starts_with(|c: char| "-?:,[]{}#&*!|>'\"%@`".contains(c))
        && !text.contains(": ")
        && !text.contains(" #")
        && !text.ends_with(':')
        && !text.chars().any(char::is_control)
        && !RESERVED.contains(&text.to_ascii_lowercase().as_str())
        && text.parse::<f64>().is_err();
    if plain {
        return text.to_string();
    }
    let mut quoted = String::from('"');
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\t' => quoted.push_str("\\t"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// Read one YAML scalar: plain, single-quoted, or double-quoted.
fn parse_scalar(raw: &str) -> Result<String, InvalidValue> {
    let raw = raw.trim();
    let rest_is_comment = |rest: &str| {
        let rest = rest.trim_start();
        rest.is_empty() || rest.starts_with('#')
    };
    if let Some(inner) = raw.strip_prefix('"') {
        let mut value = String::new();
        let mut chars = inner.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    return match inner.get(i + 1..) {
                        Some(rest) if rest_is_comment(rest) => Ok(value),
                        _ => Err(invalid("text after the closing quote")),
                    }
                }
                '\\' => match chars.next().map(|(_, e)| e) {
                    Some('\\') => value.push('\\'),
                    Some('"') => value.push('"'),
                    Some('/') => value.push('/'),
                    Some('t') => value.push('\t'),
                    Some('n') => value.push('\n'),
                    _ => return Err(invalid("unsupported escape in a quoted value")),
                },
                c => value.push(c),
            }
        }
        return Err(invalid("a quoted value has no closing quote"));
    }
    if let Some(inner) = raw.strip_prefix('\'') {
        let mut value = String::new();
        let mut chars = inner.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c != '\'' {
                value.push(c);
            } else if chars.peek().is_some_and(|(_, n)| *n == '\'') {
                value.push('\'');
                chars.next();
            } else {
                return match inner.get(i + 1..) {
                    Some(rest) if rest_is_comment(rest) => Ok(value),
                    _ => Err(invalid("text after the closing quote")),
                };
            }
        }
        return Err(invalid("a quoted value has no closing quote"));
    }
    if raw.starts_with(['|', '>', '[', '{', '&', '*', '!']) {
        return Err(invalid(format!(
            "expected a single-line value, got `{raw}`"
        )));
    }
    let value = match raw.find(" #") {
        Some(at) => raw.get(..at).unwrap_or(raw),
        None => raw,
    };
    Ok(value.trim_end().to_string())
}

/// A top-level frontmatter entry: `key: value` and its continuation lines.
struct Block<'a> {
    key: &'a str,
    value: &'a str,
    lines: Vec<&'a str>,
    raw: String,
}

enum Piece<'a> {
    Raw(String),
    Block(Block<'a>),
}

/// One `- ` list item: the text after the dash, then its indented lines.
struct Item<'a> {
    head: &'a str,
    more: Vec<&'a str>,
}

fn list_items<'a>(field: &str, lines: &[&'a str]) -> Result<Vec<Item<'a>>, NoteError> {
    let mut items: Vec<Item<'a>> = Vec::new();
    let mut dash_indent = None;
    for line in lines.iter().filter(|l| !l.trim().is_empty()) {
        let text = line.trim_start();
        let indent = line.len() - text.len();
        if text == "-" || text.starts_with("- ") {
            if dash_indent.is_some_and(|d| d != indent) {
                return Err(field_error(field, "list items are indented unevenly"));
            }
            dash_indent = Some(indent);
            items.push(Item {
                head: text.get(1..).unwrap_or_default().trim(),
                more: Vec::new(),
            });
            continue;
        }
        match (dash_indent, items.last_mut()) {
            (Some(d), Some(item)) if indent > d => item.more.push(text),
            _ => return Err(field_error(field, "expected one `- ` item per line")),
        }
    }
    Ok(items)
}

/// The `key: value` pairs of a map-shaped list item.
fn item_pairs<'a>(field: &str, item: &Item<'a>) -> Result<Vec<(&'a str, String)>, NoteError> {
    std::iter::once(item.head)
        .chain(item.more.iter().copied())
        .map(|line| {
            let (key, value) = line.split_once(':').ok_or_else(|| {
                field_error(field, format!("expected `key: value`, got `{line}`"))
            })?;
            let value = parse_scalar(value).map_err(|e| field_error(field, e))?;
            Ok((key.trim(), value))
        })
        .collect()
}

fn split_document(text: &str) -> Result<(&str, &str), NoteError> {
    let rest = text.strip_prefix("---\n").ok_or(NoteError::NotANote(
        "the file does not start with a `---` line",
    ))?;
    let mut at = 0;
    loop {
        let remaining = rest.get(at..).unwrap_or_default();
        match remaining.find('\n') {
            Some(end) => {
                if remaining.get(..end) == Some("---") {
                    let body = remaining.get(end + 1..).unwrap_or_default();
                    return Ok((rest.get(..at).unwrap_or_default(), body));
                }
                at += end + 1;
            }
            None if remaining == "---" => return Ok((rest.get(..at).unwrap_or_default(), "")),
            None => {
                return Err(NoteError::NotANote(
                    "the frontmatter has no closing `---` line",
                ))
            }
        }
    }
}

fn group_lines(frontmatter: &str) -> Result<Vec<Piece<'_>>, NoteError> {
    let mut pieces: Vec<Piece<'_>> = Vec::new();
    for (number, line) in frontmatter.split_inclusive('\n').enumerate() {
        let content = line.trim_end_matches('\n');
        let continues = content.starts_with([' ', '\t', '-']) || content.trim().is_empty();
        if continues {
            match pieces.last_mut() {
                Some(Piece::Block(block)) => {
                    block.raw.push_str(line);
                    block.lines.push(content);
                }
                Some(Piece::Raw(raw)) => raw.push_str(line),
                None => pieces.push(Piece::Raw(line.to_string())),
            }
        } else if content.starts_with('#') {
            pieces.push(Piece::Raw(line.to_string()));
        } else {
            let (key, value) = content.split_once(':').ok_or_else(|| {
                field_error(
                    "frontmatter",
                    format!("line {} is not `key: value`", number + 2),
                )
            })?;
            pieces.push(Piece::Block(Block {
                key: key.trim_end(),
                value,
                lines: Vec::new(),
                raw: line.to_string(),
            }));
        }
    }
    Ok(pieces)
}

/// Values read from Lore's keys, before the required ones are checked.
#[derive(Default)]
struct Fields {
    id: Option<NoteId>,
    note_type: Option<NoteType>,
    title: Option<String>,
    scope: Option<Scope>,
    status: Option<Status>,
    anchors: Vec<Anchor>,
    relations: Vec<Relation>,
    sources: Vec<SourceRef>,
    created: Option<NoteDate>,
    forget_after: Option<NoteDate>,
}

fn scalar_field(key: Key, block: &Block<'_>) -> Result<String, NoteError> {
    if block.lines.iter().any(|l| !l.trim().is_empty()) {
        return Err(field_error(key.name(), "expected a single-line value"));
    }
    parse_scalar(block.value).map_err(|e| field_error(key.name(), e))
}

fn typed<T>(
    key: Key,
    value: &str,
    parse: fn(&str) -> Result<T, InvalidValue>,
) -> Result<T, NoteError> {
    parse(value).map_err(|e| field_error(key.name(), e))
}

fn list_field<'a>(key: Key, block: &Block<'a>) -> Result<Vec<Item<'a>>, NoteError> {
    match parse_scalar(block.value).map_err(|e| field_error(key.name(), e)) {
        Ok(inline) if inline.is_empty() => list_items(key.name(), &block.lines),
        _ if block.value.trim() == "[]" && block.lines.iter().all(|l| l.trim().is_empty()) => {
            Ok(Vec::new())
        }
        _ => Err(field_error(
            key.name(),
            "write the list one `- ` item per line",
        )),
    }
}

fn read_anchor(index: usize, item: &Item<'_>) -> Result<Anchor, NoteError> {
    let field = format!("anchors[{index}]");
    let (mut path, mut lines, mut hash, mut commit) = (None, None, None, None);
    for (key, value) in item_pairs(&field, item)? {
        let named = |e: InvalidValue| field_error(format!("{field}.{key}"), e);
        match key {
            "path" if path.is_none() => path = Some(RelPath::parse(&value).map_err(named)?),
            "lines" if lines.is_none() => lines = Some(LineRange::parse(&value).map_err(named)?),
            "hash" if hash.is_none() => hash = Some(LineHash::parse(&value).map_err(named)?),
            "commit" if commit.is_none() => {
                commit = Some(CommitId::parse(&value).map_err(named)?);
            }
            _ => {
                return Err(field_error(
                    &field,
                    format!("unexpected or repeated key `{key}`"),
                ))
            }
        }
    }
    Ok(Anchor {
        path: path.ok_or_else(|| field_error(&field, "needs a `path`"))?,
        lines,
        hash,
        commit,
    })
}

fn read_block(key: Key, block: &Block<'_>, fields: &mut Fields) -> Result<(), NoteError> {
    match key {
        Key::Id => fields.id = Some(typed(key, &scalar_field(key, block)?, NoteId::parse)?),
        Key::Type => {
            fields.note_type = Some(typed(key, &scalar_field(key, block)?, NoteType::parse)?);
        }
        Key::Title => {
            let title = scalar_field(key, block)?;
            if title.trim().is_empty() || title.chars().any(char::is_control) {
                return Err(field_error(
                    key.name(),
                    "expected one non-empty line of text",
                ));
            }
            fields.title = Some(title);
        }
        Key::Scope => fields.scope = Some(typed(key, &scalar_field(key, block)?, Scope::parse)?),
        Key::Status => fields.status = Some(typed(key, &scalar_field(key, block)?, Status::parse)?),
        Key::Created => {
            fields.created = Some(typed(key, &scalar_field(key, block)?, NoteDate::parse)?);
        }
        Key::ForgetAfter => {
            let value = scalar_field(key, block)?;
            if !value.is_empty() {
                fields.forget_after = Some(typed(key, &value, NoteDate::parse)?);
            }
        }
        Key::Anchors => {
            for (index, item) in list_field(key, block)?.iter().enumerate() {
                fields.anchors.push(read_anchor(index, item)?);
            }
        }
        Key::Relations => {
            for (index, item) in list_field(key, block)?.iter().enumerate() {
                let field = format!("relations[{index}]");
                let pairs = item_pairs(&field, item)?;
                let [(kind, id)] = pairs.as_slice() else {
                    return Err(field_error(&field, "expected one `kind: id` pair"));
                };
                fields
                    .relations
                    .push(Relation::parse(kind, id).map_err(|e| field_error(&field, e))?);
            }
        }
        Key::Sources => {
            for (index, item) in list_field(key, block)?.iter().enumerate() {
                let field = format!("sources[{index}]");
                if !item.more.is_empty() {
                    return Err(field_error(&field, "expected `agent:session`"));
                }
                let value = parse_scalar(item.head).map_err(|e| field_error(&field, e))?;
                fields
                    .sources
                    .push(SourceRef::parse(&value).map_err(|e| field_error(&field, e))?);
            }
        }
    }
    Ok(())
}

fn parse_note(text: &str, fill: Option<Fill>) -> Result<Draft, NoteError> {
    let (frontmatter, body) = split_document(text)?;
    if frontmatter.contains('\r') {
        return Err(NoteError::NotANote("CRLF line endings are not supported"));
    }
    let mut extra = Frontmatter::default();
    let mut fields = Fields::default();
    for piece in group_lines(frontmatter)? {
        let block = match piece {
            Piece::Raw(raw) => {
                extra.slots.push(Slot::Raw(raw));
                continue;
            }
            Piece::Block(block) => block,
        };
        let Some(key) = Key::from_name(block.key) else {
            extra.slots.push(Slot::Raw(block.raw));
            continue;
        };
        if extra.has(key) {
            return Err(NoteError::Duplicate(key.name()));
        }
        read_block(key, &block, &mut fields)?;
        extra.slots.push(Slot::Known(key));
        let trailing: String = block
            .raw
            .split_inclusive('\n')
            .rev()
            .take_while(|l| l.trim().is_empty())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if !trailing.is_empty() {
            extra.slots.push(Slot::Raw(trailing));
        }
    }

    let (mut id_filled, mut created_filled) = (false, false);
    if let Some(fill) = fill {
        if fields.id.is_none() {
            fields.id = Some(fill.id);
            id_filled = true;
        }
        if fields.created.is_none() {
            fields.created = Some(fill.today);
            created_filled = true;
        }
        fields.status.get_or_insert(Status::Active);
        fields.scope.get_or_insert(Scope::Global);
        for key in [Key::Id, Key::Scope, Key::Status, Key::Created] {
            extra.insert(key);
        }
    }

    let note = Note {
        id: fields.id.ok_or(NoteError::Missing("id"))?,
        note_type: fields.note_type.ok_or(NoteError::Missing("type"))?,
        title: fields.title.ok_or(NoteError::Missing("title"))?,
        scope: fields.scope.ok_or(NoteError::Missing("scope"))?,
        status: fields.status.ok_or(NoteError::Missing("status"))?,
        anchors: fields.anchors,
        relations: fields.relations,
        sources: fields.sources,
        created: fields.created.ok_or(NoteError::Missing("created"))?,
        forget_after: fields.forget_after,
        body: body.to_string(),
        extra,
    };
    Ok(Draft {
        note,
        id_filled,
        created_filled,
    })
}
