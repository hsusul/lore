//! A person's review of the inbox: accept, reject, and retire.
//!
//! Accepting is the only path from a proposal into the vault's history. Each
//! accept or retire holds the vault's write lock, writes one note file, and
//! commits only that path, so a person's uncommitted edits elsewhere in the
//! vault are never swept into Lore's commit. A note path that already has
//! uncommitted changes Lore did not make is refused.
//!
//! Decisions are appended to `<vault>/.lore/decisions.jsonl`; inbox files are
//! never rewritten. A proposal is pending until a decision names it.
//!
//! Accept and retire converge: a run killed at any point, then run again,
//! leaves the same files and the same number of commits as one clean run. The
//! commit is the source of truth and the decision line is a cache of it, so a
//! lost decision line is recovered from the note file or from the commit's
//! `Lore-Proposal` trailer.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};

use serde_json::{json, Value};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::inbox::{self, append_timeline, InboxError, Proposal, Skipped, TIMELINE_HEADING};
use crate::index::Index;
use crate::note::{Note, NoteDate, NoteId, Relation, Status};
use crate::vault::{check_id_free, write_atomically, Added, Vault, VaultError};

const DECISIONS_FILE: &str = "decisions.jsonl";

#[derive(Debug, Error)]
pub enum ReviewError {
    #[error("no proposal `{0}` in the inbox")]
    NoProposal(String),
    #[error("proposal `{0}` was already {1}")]
    Decided(String, &'static str),
    #[error("proposal `{id}` at {at} is {reason}")]
    Refused {
        id: String,
        at: String,
        reason: String,
    },
    #[error("proposal `{0}` appears more than once in the inbox")]
    DuplicateProposal(String),
    #[error("{0} has uncommitted changes Lore did not make")]
    Dirty(String),
    #[error("`{0}` is not a valid slug: use lowercase letters, digits, and single `-`")]
    BadSlug(String),
    #[error(transparent)]
    Inbox(#[from] InboxError),
    #[error(transparent)]
    Vault(#[from] VaultError),
}

/// What a person decided about a proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Accepted { note: String, id: NoteId },
    Rejected { reason: String },
}

/// The proposals no decision names yet, oldest first, and the inbox lines
/// that are not valid proposals.
#[derive(Debug)]
pub struct Pending {
    pub proposals: Vec<Proposal>,
    pub skipped: Vec<Skipped>,
}

/// What `accept` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// The vault path of the note written or rewritten.
    pub rel: String,
    pub id: NoteId,
    /// True when the proposal rewrote an existing note (`relation: updates`).
    pub updated: bool,
    pub outcome: Added,
}

/// The pending proposals.
pub fn inbox(vault: &Vault) -> Result<Pending, ReviewError> {
    let (mut proposals, skipped) = inbox::proposals(vault)?;
    let decisions = decisions(vault)?;
    proposals.retain(|p| !decisions.contains_key(&p.id));
    if !proposals.is_empty() {
        let committed = proposal_commits(vault, None)?;
        proposals.retain(|p| !committed.iter().any(|(id, _)| *id == p.id));
    }
    proposals.sort_by(|a, b| (a.proposed_at, &a.id).cmp(&(b.proposed_at, &b.id)));
    Ok(Pending { proposals, skipped })
}

/// Accept proposal `id`: write it as a new note at `<scope>/<slug>.md`, with
/// `as_slug` overriding the slug its title gives, or, for `relation: updates`,
/// rewrite the target note. One commit. Running it again changes nothing.
pub fn accept(vault: &Vault, id: &str, as_slug: Option<&str>) -> Result<Accepted, ReviewError> {
    if let Some(slug) = as_slug {
        check_slug(slug)?;
    }
    let proposal = find_proposal(vault, id)?;
    let updates = matches!(proposal.relation, Some(Relation::Updates(_)));
    // Check the log before looking for the target: an accepted update stays
    // accepted even when its target was deleted by hand since.
    let decided = |vault: &Vault| -> Result<Option<Accepted>, ReviewError> {
        match decisions(vault)?.remove(&proposal.id) {
            Some(Decision::Accepted { note, id: note_id }) => Ok(Some(Accepted {
                rel: note,
                id: note_id,
                updated: updates,
                outcome: Added::Unchanged,
            })),
            Some(Decision::Rejected { .. }) => {
                Err(ReviewError::Decided(id.to_string(), "rejected"))
            }
            None => Ok(None),
        }
    };
    if let Some(accepted) = decided(vault)? {
        return Ok(accepted);
    }
    let target = match &proposal.relation {
        Some(Relation::Updates(target)) => Some(vault.find(target.as_str())?),
        _ => None,
    };
    let _lock = vault.lock()?;
    if let Some(accepted) = decided(vault)? {
        return Ok(accepted);
    }
    let today = NoteDate::today();
    let (accepted, note) = match target {
        Some(rel) => {
            let (accepted, note) = update_locked(vault, &proposal, rel, today)?;
            (accepted, Some(note))
        }
        None => create_locked(vault, &proposal, as_slug, today)?,
    };
    record(
        vault,
        &proposal.id,
        &Decision::Accepted {
            note: accepted.rel.clone(),
            id: accepted.id.clone(),
        },
    )?;
    if let Some(note) = note {
        upsert_index(vault, &accepted.rel, &note)?;
    }
    Ok(accepted)
}

/// Reject proposal `id`. Writes only the decision log, never the vault's
/// files or history. Returns false when it was already rejected.
pub fn reject(vault: &Vault, id: &str, reason: &str) -> Result<bool, ReviewError> {
    inbox::check_line("reason", reason)?;
    let reason = inbox::redact(reason)?;
    let proposal = find_proposal(vault, id)?;
    let _lock = vault.lock()?;
    match decisions(vault)?.remove(&proposal.id) {
        Some(Decision::Rejected { .. }) => Ok(false),
        Some(Decision::Accepted { .. }) => Err(ReviewError::Decided(id.to_string(), "accepted")),
        None => {
            if let Some((note, note_id)) = accepted_commit(vault, &proposal.id)? {
                record(
                    vault,
                    &proposal.id,
                    &Decision::Accepted { note, id: note_id },
                )?;
                return Err(ReviewError::Decided(id.to_string(), "accepted"));
            }
            record(vault, &proposal.id, &Decision::Rejected { reason })?;
            Ok(true)
        }
    }
}

/// Retire the note named by `key` (id, slug, or path): `status: retired` and a
/// Timeline line with the reason. One commit. Running it again changes nothing.
pub fn retire(vault: &Vault, key: &str, reason: &str) -> Result<(String, Added), ReviewError> {
    inbox::check_line("reason", reason)?;
    let reason = inbox::redact(reason)?;
    let rel = vault.find(key)?;
    let _lock = vault.lock()?;
    let text = vault.read(&rel)?;
    let current = Note::parse(&text).map_err(VaultError::from)?;
    check_id_free(&vault.refresh_locked()?, &current.id, &rel)?;
    let committed = vault.is_committed(&rel)?;
    let today = NoteDate::today();
    let retired = |note: Note, date: NoteDate| {
        let mut note = note;
        note.status = Status::Retired;
        note.body = append_timeline(&note.body, &format!("- {date} retired: {reason}"));
        note
    };
    let note = match (current.status, committed) {
        (Status::Retired, true) => {
            upsert_index(vault, &rel, &current)?;
            return Ok((rel, Added::Unchanged));
        }
        (Status::Active, true) => {
            let note = retired(current, today);
            write_atomically(&vault.root().join(&rel), &note.render())?;
            note
        }
        (_, false) => {
            let head =
                committed_text(vault, &rel)?.ok_or_else(|| ReviewError::Dirty(rel.clone()))?;
            let base = Note::parse(&head).map_err(|_| ReviewError::Dirty(rel.clone()))?;
            let date = last_timeline_date(&text, today);
            if base.status != Status::Active || retired(base, date).render() != text {
                return Err(ReviewError::Dirty(rel));
            }
            current
        }
    };
    commit(
        vault,
        &rel,
        &format!("mem: retire {}\n\n{reason}", slug_of(&rel)),
    )?;
    upsert_index(vault, &rel, &note)?;
    Ok((rel, Added::Committed))
}

fn create_locked(
    vault: &Vault,
    proposal: &Proposal,
    as_slug: Option<&str>,
    today: NoteDate,
) -> Result<(Accepted, Option<Note>), ReviewError> {
    // A commit already accepted it and only the decision line was lost. The
    // note may since be retired or written under another `--as` name.
    if let Some((rel, id)) = accepted_commit(vault, &proposal.id)? {
        let current = read_optional(vault, &rel)?.and_then(|text| Note::parse(&text).ok());
        let accepted = Accepted {
            rel,
            id,
            updated: false,
            outcome: Added::Unchanged,
        };
        return Ok((accepted, current));
    }
    let mut note = proposal.note(NoteId::generate(), today);
    let rel = match as_slug {
        Some(slug) => format!("{}/{slug}.md", note.scope.dir()),
        None => note.rel_path(),
    };
    let path = vault.root().join(&rel);
    let outcome = match read_optional(vault, &rel)? {
        None => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| VaultError::Io(format!("create {rel}"), e))?;
            }
            write_atomically(&path, &note.render())?;
            None
        }
        Some(existing) => {
            let current = Note::parse(&existing).map_err(|_| VaultError::Conflict(rel.clone()))?;
            note = proposal.note(current.id.clone(), current.created);
            if note.render() != existing {
                return Err(VaultError::Conflict(rel).into());
            }
            check_id_free(&vault.refresh_locked()?, &note.id, &rel)?;
            vault.is_committed(&rel)?.then_some(Added::Unchanged)
        }
    };
    let outcome = match outcome {
        Some(unchanged) => unchanged,
        None => {
            commit(vault, &rel, &commit_message("accept", &rel, proposal))?;
            Added::Committed
        }
    };
    let accepted = Accepted {
        rel,
        id: note.id.clone(),
        updated: false,
        outcome,
    };
    Ok((accepted, Some(note)))
}

/// The target with the proposal's statement in place of its own, the
/// proposal's anchors when it has any, its source added, and a Timeline line.
/// Title, type, path, relations, unknown keys, and the Timeline so far stay.
fn apply_update(target: Note, proposal: &Proposal, date: NoteDate) -> Note {
    let mut note = target;
    let history = note
        .body
        .split_inclusive('\n')
        .position(|l| l.trim_end() == TIMELINE_HEADING)
        .map(|at| note.body.split_inclusive('\n').skip(at).collect::<String>())
        .unwrap_or_default();
    let mut body = proposal.body.clone();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    if !history.is_empty() {
        body.push('\n');
        body.push_str(&history);
    }
    note.body = append_timeline(&body, &proposal.timeline_line(date));
    if !proposal.anchors.is_empty() {
        note.anchors = proposal.anchors.clone();
    }
    let source = proposal.source();
    if !note.sources.contains(&source) {
        note.sources.push(source);
    }
    note
}

fn update_locked(
    vault: &Vault,
    proposal: &Proposal,
    rel: String,
    today: NoteDate,
) -> Result<(Accepted, Note), ReviewError> {
    let same_scope = |target: &Note| {
        if target.scope == proposal.scope {
            Ok(())
        } else {
            Err(InboxError::Field {
                field: "scope".into(),
                problem: format!("the target {rel} is in scope {}", target.scope),
            })
        }
    };
    let text = vault.read(&rel)?;
    let note = if vault.is_committed(&rel)? {
        let current = Note::parse(&text).map_err(VaultError::from)?;
        same_scope(&current)?;
        if history_names(vault, &rel, &proposal.id)? {
            let accepted = Accepted {
                rel,
                id: current.id.clone(),
                updated: true,
                outcome: Added::Unchanged,
            };
            return Ok((accepted, current));
        }
        let note = apply_update(current, proposal, today);
        write_atomically(&vault.root().join(&rel), &note.render())?;
        note
    } else {
        let head = committed_text(vault, &rel)?.ok_or_else(|| ReviewError::Dirty(rel.clone()))?;
        let base = Note::parse(&head).map_err(|_| ReviewError::Dirty(rel.clone()))?;
        same_scope(&base)?;
        let note = apply_update(base, proposal, last_timeline_date(&text, today));
        if note.render() != text {
            return Err(ReviewError::Dirty(rel));
        }
        note
    };
    commit(vault, &rel, &commit_message("update", &rel, proposal))?;
    let accepted = Accepted {
        rel,
        id: note.id.clone(),
        updated: true,
        outcome: Added::Committed,
    };
    Ok((accepted, note))
}

/// The date of the last `- YYYY-MM-DD` line in `text`, which is the Timeline
/// line an interrupted write added. A write from before midnight is then
/// still recognised after it. `today` when there is none.
fn last_timeline_date(text: &str, today: NoteDate) -> NoteDate {
    text.lines()
        .rev()
        .find_map(|line| {
            let date = line.strip_prefix("- ")?.get(..10)?;
            NoteDate::parse(date).ok()
        })
        .unwrap_or(today)
}

fn commit_message(verb: &str, rel: &str, proposal: &Proposal) -> String {
    format!(
        "mem: {verb} {}\n\nLore-Proposal: {}\nLore-Source: {}",
        slug_of(rel),
        proposal.id.as_str(),
        proposal.source()
    )
}

fn commit(vault: &Vault, rel: &str, message: &str) -> Result<(), VaultError> {
    vault.git(&["add", "--", rel])?;
    vault.git(&["commit", "-q", "-m", message, "--", rel])?;
    Ok(())
}

/// Whether any commit that touched `rel` came from proposal `id`. Not only the
/// newest: a retire or a later update may have landed after it.
fn history_names(vault: &Vault, rel: &str, id: &NoteId) -> Result<bool, VaultError> {
    let trailer = format!("Lore-Proposal: {}", id.as_str());
    let grep = format!("--grep={trailer}");
    let bodies = vault.git(&["log", "-F", &grep, "--format=%B", "--", rel])?;
    Ok(bodies.lines().any(|line| line == trailer))
}

/// Each commit whose message carries a `Lore-Proposal:` trailer, oldest first:
/// the proposal id and the commit. Only proposal `id` when it is given.
fn proposal_commits(
    vault: &Vault,
    id: Option<&NoteId>,
) -> Result<Vec<(NoteId, String)>, VaultError> {
    const TRAILER: &str = "Lore-Proposal: ";
    let grep = format!("--grep={TRAILER}{}", id.map_or("", NoteId::as_str));
    let log = vault.git(&["log", "--reverse", "-F", &grep, "--format=%x1e%H%n%B"])?;
    let mut out = Vec::new();
    for record in log.split('\x1e') {
        let Some((sha, body)) = record.split_once('\n') else {
            continue;
        };
        for line in body.lines() {
            let found = line
                .strip_prefix(TRAILER)
                .and_then(|v| NoteId::parse(v).ok());
            if let Some(found) = found.filter(|f| id.is_none_or(|id| id == f)) {
                out.push((found, sha.to_string()));
            }
        }
    }
    Ok(out)
}

/// The path and note id of the first commit that accepted proposal `id`, as
/// that commit left them. `None` when no commit names it.
fn accepted_commit(vault: &Vault, id: &NoteId) -> Result<Option<(String, NoteId)>, VaultError> {
    let Some((_, sha)) = proposal_commits(vault, Some(id))?.into_iter().next() else {
        return Ok(None);
    };
    let names = vault.git(&["show", "--name-only", "--format=", &sha])?;
    let rel = names.lines().next().unwrap_or_default().to_string();
    let note = Note::parse(&vault.git(&["show", &format!("{sha}:{rel}")])?)?;
    Ok(Some((rel, note.id)))
}

/// The committed text of `rel`, or `None` when it was never committed.
fn committed_text(vault: &Vault, rel: &str) -> Result<Option<String>, VaultError> {
    if vault
        .git(&["ls-files", "--error-unmatch", "--", rel])
        .is_err()
    {
        return Ok(None);
    }
    vault.git(&["show", &format!("HEAD:{rel}")]).map(Some)
}

fn read_optional(vault: &Vault, rel: &str) -> Result<Option<String>, VaultError> {
    match fs::read_to_string(vault.root().join(rel)) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(VaultError::Io(format!("read {rel}"), e)),
    }
}

fn upsert_index(vault: &Vault, rel: &str, note: &Note) -> Result<(), VaultError> {
    if let Some(index) = Index::open(&vault.lore_dir())? {
        index.upsert(rel, note)?;
    }
    Ok(())
}

fn slug_of(rel: &str) -> &str {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.strip_suffix(".md").unwrap_or(name)
}

fn check_slug(slug: &str) -> Result<(), ReviewError> {
    let ok = (1..=64).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--");
    if ok {
        Ok(())
    } else {
        Err(ReviewError::BadSlug(slug.to_string()))
    }
}

fn find_proposal(vault: &Vault, id: &str) -> Result<Proposal, ReviewError> {
    let wanted = NoteId::parse(id).map_err(|e| InboxError::Field {
        field: "id".into(),
        problem: e.to_string(),
    })?;
    let (proposals, skipped) = inbox::proposals(vault)?;
    if let Some(refused) = skipped.into_iter().find(|s| s.id.as_ref() == Some(&wanted)) {
        return Err(ReviewError::Refused {
            id: id.to_string(),
            at: refused.at,
            reason: refused.reason,
        });
    }
    let mut matches = proposals.into_iter().filter(|p| p.id == wanted);
    match (matches.next(), matches.next()) {
        (Some(proposal), None) => Ok(proposal),
        (Some(_), Some(_)) => Err(ReviewError::DuplicateProposal(id.to_string())),
        (None, _) => Err(ReviewError::NoProposal(id.to_string())),
    }
}

/// Every decision, last one per proposal winning. A line that does not parse,
/// such as one torn by a crash, is ignored: the commit it would describe is
/// found again from the vault.
pub fn decisions(vault: &Vault) -> Result<HashMap<NoteId, Decision>, VaultError> {
    let path = vault.lore_dir().join(DECISIONS_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(VaultError::Io(format!("read {DECISIONS_FILE}"), e)),
    };
    let mut out = HashMap::new();
    for line in text.lines() {
        if let Some((id, decision)) = parse_decision(line) {
            out.insert(id, decision);
        }
    }
    Ok(out)
}

fn parse_decision(line: &str) -> Option<(NoteId, Decision)> {
    let value: Value = serde_json::from_str(line).ok()?;
    let get = |key: &str| value.get(key).and_then(Value::as_str);
    let id = NoteId::parse(get("id")?).ok()?;
    let decision = match get("decision")? {
        "accepted" => Decision::Accepted {
            note: get("note")?.to_string(),
            id: NoteId::parse(get("note_id")?).ok()?,
        },
        "rejected" => Decision::Rejected {
            reason: get("reason")?.to_string(),
        },
        _ => return None,
    };
    Some((id, decision))
}

fn record(vault: &Vault, id: &NoteId, decision: &Decision) -> Result<(), VaultError> {
    let at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let value = match decision {
        Decision::Accepted { note, id: note_id } => json!({
            "id": id.as_str(), "decision": "accepted", "note": note,
            "note_id": note_id.as_str(), "at": at,
        }),
        Decision::Rejected { reason } => json!({
            "id": id.as_str(), "decision": "rejected", "reason": reason, "at": at,
        }),
    };
    let io = |e| VaultError::Io(format!("append to {DECISIONS_FILE}"), e);
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(vault.lore_dir().join(DECISIONS_FILE))
        .map_err(io)?;
    // A line torn by a crash has no newline. Start a fresh line after it, so
    // this decision is not glued onto the torn one and lost with it.
    let mut line = if ends_torn(&mut file).map_err(io)? {
        String::from("\n")
    } else {
        String::new()
    };
    line.push_str(&value.to_string());
    line.push('\n');
    file.write_all(line.as_bytes()).map_err(io)
}

/// Whether the file is not empty and its last byte is not a newline.
pub(crate) fn ends_torn(file: &mut fs::File) -> io::Result<bool> {
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8];
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}
