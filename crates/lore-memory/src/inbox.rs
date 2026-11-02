//! Agent proposals: the only thing an agent writes into a vault.
//!
//! A proposal is one JSON line in `<vault>/_inbox/<agent>-<session>.jsonl`.
//! Each agent session appends to its own file with one `O_APPEND` write per
//! line, so parallel agents never share a write target and need no lock.
//! `_inbox/` is gitignored. A proposal reaches the vault's history only when a
//! person accepts it ([`crate::review::accept`]).
//!
//! Title, body, and anchor paths pass through `lore-core`'s secret redaction
//! before anything is written, and a proposal is only written when the note it
//! would become survives a render and parse unchanged.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};

use lore_core::secrets;
use serde_json::{json, Map, Value};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::note::{
    AgentKind, Anchor, CommitId, Frontmatter, LineHash, LineRange, Note, NoteDate, NoteId,
    NoteType, RelPath, Relation, Scope, SessionId, SourceRef, Status,
};
use crate::vault::{Vault, VaultError};

/// The inbox directory inside the vault. Gitignored.
pub const INBOX_DIR: &str = "_inbox";
/// The heading that starts a note's append-only history.
pub const TIMELINE_HEADING: &str = "## Timeline";

/// A note an agent wants a person to accept.
///
/// `scope` is not in the design's field list. Without it every accepted note
/// would be global, and anchors only mean something inside one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub id: NoteId,
    pub agent: AgentKind,
    pub session: SessionId,
    pub note_type: NoteType,
    pub scope: Scope,
    pub title: String,
    pub body: String,
    pub anchors: Vec<Anchor>,
    pub relation: Option<Relation>,
    pub proposed_at: OffsetDateTime,
}

/// A line in the inbox that is not a valid proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// `_inbox/<file>:<line>`.
    pub at: String,
    /// The proposal id, when the line parsed but was refused.
    pub id: Option<NoteId>,
    pub reason: String,
}

#[derive(Debug, Error)]
pub enum InboxError {
    #[error("`{field}`: {problem}")]
    Field { field: String, problem: String },
    #[error("the secret scan failed, so nothing was written")]
    ScanFailed,
    #[error("{0}: {1}")]
    Io(String, #[source] io::Error),
    #[error(transparent)]
    Vault(#[from] VaultError),
}

fn field(name: impl Into<String>, problem: impl ToString) -> InboxError {
    InboxError::Field {
        field: name.into(),
        problem: problem.to_string(),
    }
}

fn io_error(what: impl Into<String>) -> impl FnOnce(io::Error) -> InboxError {
    let what = what.into();
    move |error| InboxError::Io(what, error)
}

/// `text` with every span the secret scanner flags replaced by a mask.
pub fn redact(text: &str) -> Result<String, InboxError> {
    let findings = secrets::scan(text).map_err(|_| InboxError::ScanFailed)?;
    Ok(secrets::redact(text, &findings))
}

/// A single line of text a person or agent typed: a title or a reason.
pub fn check_line(name: &str, text: &str) -> Result<(), InboxError> {
    if text.trim().is_empty() {
        return Err(field(name, "must not be empty"));
    }
    if text.chars().any(char::is_control) {
        return Err(field(name, "must be one line without control characters"));
    }
    Ok(())
}

impl Proposal {
    /// Where the proposal lives: `_inbox/<agent>-<session>.jsonl`, with the
    /// session lowercased, so sessions that differ only in case share one
    /// file on every file system, not only on case-insensitive ones.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!(
            "{}-{}.jsonl",
            self.agent.as_str(),
            self.session.as_str().to_ascii_lowercase()
        )
    }

    #[must_use]
    pub fn source(&self) -> SourceRef {
        SourceRef {
            agent: self.agent,
            session: self.session.clone(),
        }
    }

    /// The Timeline line an accepted proposal adds on `date`.
    #[must_use]
    pub fn timeline_line(&self, date: NoteDate) -> String {
        format!("- {date} accepted from {}", self.source())
    }

    /// The new note this proposal becomes when accepted on `date` with `id`.
    #[must_use]
    pub fn note(&self, id: NoteId, date: NoteDate) -> Note {
        let relations = match &self.relation {
            Some(Relation::Updates(_)) | None => Vec::new(),
            Some(other) => vec![other.clone()],
        };
        Note {
            id,
            note_type: self.note_type,
            title: self.title.clone(),
            scope: self.scope.clone(),
            repo: None,
            status: Status::Active,
            anchors: self.anchors.clone(),
            relations,
            sources: vec![self.source()],
            created: date,
            forget_after: None,
            body: append_timeline(&self.body, &self.timeline_line(date)),
            extra: Frontmatter::default(),
        }
    }

    /// The same proposal with title, body, and anchor paths redacted. A
    /// session id the scanner flags is refused, not masked, because it names
    /// the inbox file.
    pub fn redacted(mut self) -> Result<Self, InboxError> {
        let session = secrets::scan(self.session.as_str()).map_err(|_| InboxError::ScanFailed)?;
        if !session.is_empty() {
            return Err(field("session", "looks like a secret"));
        }
        self.title = redact(&self.title)?;
        self.body = redact(&self.body)?;
        for (i, anchor) in self.anchors.iter_mut().enumerate() {
            let path = redact(anchor.path.as_str())?;
            anchor.path =
                RelPath::parse(&path).map_err(|e| field(format!("anchors[{i}].path"), e))?;
        }
        Ok(self)
    }

    /// Refuse anything that would not become a valid note byte for byte.
    pub fn validate(&self) -> Result<(), InboxError> {
        check_line("title", &self.title)?;
        if self.body.trim().is_empty() {
            return Err(field("body", "must not be empty"));
        }
        if self.body.contains('\r') {
            return Err(field("body", "CRLF line endings are not allowed"));
        }
        if self.body.lines().any(|l| l.trim_end() == TIMELINE_HEADING) {
            return Err(field(
                "body",
                "the `## Timeline` section is Lore's to write",
            ));
        }
        if self.relation.as_ref().map(Relation::target) == Some(&self.id) {
            return Err(field("relation", "a proposal cannot relate to itself"));
        }
        let note = self.note(self.id.clone(), NoteDate::today());
        let parsed = Note::parse(&note.render()).map_err(|e| field("note", e))?;
        let parsed = Note {
            extra: Frontmatter::default(),
            ..parsed
        };
        if parsed == note {
            Ok(())
        } else {
            Err(field("title", "does not survive a render and parse"))
        }
    }

    /// The inbox line, without its newline.
    #[must_use]
    pub fn to_line(&self) -> String {
        let anchors: Vec<Value> = self.anchors.iter().map(anchor_json).collect();
        let mut line = json!({
            "id": self.id.as_str(),
            "agent": self.agent.as_str(),
            "session": self.session.as_str(),
            "type": self.note_type.as_str(),
            "scope": self.scope.to_string(),
            "title": self.title,
            "body": self.body,
            "anchors": anchors,
            "proposed_at": self.proposed_at.format(&Rfc3339).unwrap_or_default(),
        });
        if let (Some(relation), Some(map)) = (&self.relation, line.as_object_mut()) {
            map.insert(
                "relation".into(),
                json!({ relation.kind(): relation.target().as_str() }),
            );
        }
        line.to_string()
    }

    /// Parse one inbox line.
    pub fn from_line(line: &str) -> Result<Self, InboxError> {
        let value: Value = serde_json::from_str(line).map_err(|e| field("line", e))?;
        let map = value
            .as_object()
            .ok_or_else(|| field("line", "expected a JSON object"))?;
        let text = |key: &str| -> Result<&str, InboxError> {
            map.get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| field(key, "missing or not a string"))
        };
        let anchors = match map.get("anchors") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(i, item)| parse_anchor(i, item))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(field("anchors", "expected a list")),
        };
        let relation = match map.get("relation") {
            None | Some(Value::Null) => None,
            Some(Value::Object(rel)) if rel.len() == 1 => {
                let (kind, target) = rel
                    .iter()
                    .next()
                    .ok_or_else(|| field("relation", "is empty"))?;
                let target = target
                    .as_str()
                    .ok_or_else(|| field("relation", "the target must be a note id"))?;
                Some(Relation::parse(kind, target).map_err(|e| field("relation", e))?)
            }
            Some(_) => return Err(field("relation", "expected one `kind: id` pair")),
        };
        Ok(Self {
            id: NoteId::parse(text("id")?).map_err(|e| field("id", e))?,
            agent: AgentKind::parse(text("agent")?).map_err(|e| field("agent", e))?,
            session: SessionId::parse(text("session")?).map_err(|e| field("session", e))?,
            note_type: NoteType::parse(text("type")?).map_err(|e| field("type", e))?,
            scope: Scope::parse(text("scope")?).map_err(|e| field("scope", e))?,
            title: text("title")?.to_string(),
            body: text("body")?.to_string(),
            anchors,
            relation,
            proposed_at: OffsetDateTime::parse(text("proposed_at")?, &Rfc3339)
                .map_err(|e| field("proposed_at", e))?,
        })
    }
}

fn anchor_json(anchor: &Anchor) -> Value {
    let mut map = Map::new();
    map.insert("path".into(), anchor.path.as_str().into());
    if let Some(lines) = anchor.lines {
        map.insert("lines".into(), lines.to_string().into());
    }
    if let Some(hash) = &anchor.hash {
        map.insert("hash".into(), hash.as_str().into());
    }
    if let Some(commit) = &anchor.commit {
        map.insert("commit".into(), commit.as_str().into());
    }
    Value::Object(map)
}

fn parse_anchor(i: usize, item: &Value) -> Result<Anchor, InboxError> {
    let name = |key: &str| format!("anchors[{i}].{key}");
    let get = |key: &str| -> Result<Option<&str>, InboxError> {
        match item.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(field(name(key), "expected a string")),
        }
    };
    let path = get("path")?.ok_or_else(|| field(name("path"), "is required"))?;
    Ok(Anchor {
        path: RelPath::parse(path).map_err(|e| field(name("path"), e))?,
        lines: get("lines")?
            .map(LineRange::parse)
            .transpose()
            .map_err(|e| field(name("lines"), e))?,
        hash: get("hash")?
            .map(LineHash::parse)
            .transpose()
            .map_err(|e| field(name("hash"), e))?,
        commit: get("commit")?
            .map(CommitId::parse)
            .transpose()
            .map_err(|e| field(name("commit"), e))?,
    })
}

/// `body` with `line` appended to its `## Timeline`, which is created when the
/// body has none. The body always ends with a newline afterwards.
#[must_use]
pub fn append_timeline(body: &str, line: &str) -> String {
    let mut out = body.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.lines().any(|l| l.trim_end() == TIMELINE_HEADING) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(TIMELINE_HEADING);
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    out
}

/// Redact, validate, and append `proposal` to its session's inbox file.
/// Returns the proposal as written.
pub fn propose(vault: &Vault, proposal: Proposal) -> Result<Proposal, InboxError> {
    let proposal = proposal.redacted()?;
    proposal.validate()?;
    let dir = vault.root().join(INBOX_DIR);
    fs::create_dir_all(&dir).map_err(io_error("create _inbox"))?;
    let name = proposal.file_name();
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(dir.join(&name))
        .map_err(io_error(format!("open {INBOX_DIR}/{name}")))?;
    // Start a fresh line after one torn by a crash, so this proposal is not
    // glued onto it and lost with it.
    let mut line = if crate::review::ends_torn(&mut file)
        .map_err(io_error(format!("read {INBOX_DIR}/{name}")))?
    {
        String::from("\n")
    } else {
        String::new()
    };
    line.push_str(&proposal.to_line());
    line.push('\n');
    file.write_all(line.as_bytes())
        .map_err(io_error(format!("append to {INBOX_DIR}/{name}")))?;
    Ok(proposal)
}

/// Every proposal in the inbox, decided or not, in file then line order, and
/// the lines that are not valid proposals.
pub fn proposals(vault: &Vault) -> Result<(Vec<Proposal>, Vec<Skipped>), InboxError> {
    let dir = vault.root().join(INBOX_DIR);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
        Err(e) => return Err(io_error("list _inbox")(e)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let name = entry
            .map_err(io_error("list _inbox"))?
            .file_name()
            .to_string_lossy()
            .into_owned();
        if name.ends_with(".jsonl") && !name.starts_with('.') {
            names.push(name);
        }
    }
    names.sort();
    let mut found = Vec::new();
    let mut skipped = Vec::new();
    for name in names {
        let text = fs::read_to_string(dir.join(&name))
            .map_err(io_error(format!("read {INBOX_DIR}/{name}")))?;
        for (number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let at = format!("{INBOX_DIR}/{name}:{}", number + 1);
            match Proposal::from_line(line) {
                Ok(p) if p.file_name() == name.to_ascii_lowercase() => match refusal(&p) {
                    None => found.push(p),
                    Some(problem) => skipped.push(Skipped {
                        at,
                        id: Some(p.id),
                        reason: format!("refused: {problem}"),
                    }),
                },
                Ok(_) => skipped.push(Skipped {
                    at,
                    id: None,
                    reason: "the agent and session do not match the file name".into(),
                }),
                Err(error) => skipped.push(Skipped {
                    at,
                    id: None,
                    reason: error.to_string(),
                }),
            }
        }
    }
    Ok((found, skipped))
}

/// Why a parsed inbox line cannot be trusted: it was not written by
/// [`propose`], because it would change under redaction or fails validation.
fn refusal(proposal: &Proposal) -> Option<String> {
    match proposal.clone().redacted() {
        Ok(redacted) if redacted == *proposal => {}
        Ok(_) => return Some("it holds text the secret scan masks".into()),
        Err(error) => return Some(error.to_string()),
    }
    proposal.validate().err().map(|error| error.to_string())
}
