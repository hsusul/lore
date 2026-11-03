//! A proposal's receipt: the archived session it came from, and the passage
//! of that session that shares the most words with what it proposes.
//!
//! Only the redacted search projection is read, the same text `search` and
//! the MCP tools return, so a receipt never shows a span the secret scan
//! masked. The archive is opened read only and never created.

use std::collections::BTreeSet;
use std::path::Path;

use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::{query, search, storage};
use rusqlite::Connection;
use serde_json::{json, Value};
use time::OffsetDateTime;

/// Characters of excerpt shown.
const EXCERPT_CHARS: usize = 160;
/// Documents read from one session, so a huge session stays bounded.
const MAX_DOCUMENTS: usize = 2_000;
const PAGE: i64 = 200;

/// Where a proposal's session was found, if anywhere.
pub enum Receipt {
    NoArchive,
    /// The session id as the proposal gives it.
    NotArchived(String),
    Found {
        session: String,
        agent: String,
        started: Option<String>,
        excerpt: Option<String>,
    },
}

/// The archive, if there is one.
pub struct Archive(Option<Connection>);

impl Archive {
    pub fn open(dir: Option<&Path>) -> Self {
        Self(dir.and_then(|dir| storage::open_read_only(&dir.join(ARCHIVE_DB_FILENAME)).ok()))
    }

    /// The receipt for `session`, an archive id or the agent's own id, with
    /// the excerpt that best matches `text`.
    pub fn receipt(&self, session: &str, text: &str) -> Receipt {
        let Some(conn) = &self.0 else {
            return Receipt::NoArchive;
        };
        let Some(owner) = query::find_session(conn, session).ok().flatten() else {
            return Receipt::NotArchived(session.to_owned());
        };
        let started = query::get_session(conn, &owner.archive_id)
            .ok()
            .flatten()
            .and_then(|s| s.summary.started_at)
            .and_then(date);
        let excerpt = best_passage(conn, &owner.archive_id, &words(text));
        Receipt::Found {
            session: owner.archive_id,
            agent: owner.agent_id,
            started,
            excerpt,
        }
    }
}

impl Receipt {
    /// The indented line under a proposal in `mem inbox`.
    pub fn line(&self) -> String {
        match self {
            Receipt::NoArchive => "    receipt: no archive yet; `lorectl scan` builds one".into(),
            Receipt::NotArchived(session) => {
                format!("    receipt: session {session} is not in the archive")
            }
            Receipt::Found {
                session,
                agent,
                started,
                excerpt,
            } => {
                let mut line = format!("    from {agent} session {session}");
                if let Some(started) = started {
                    line.push_str(&format!(", {started}"));
                }
                if let Some(excerpt) = excerpt {
                    line.push_str(&format!(": \"{excerpt}\""));
                }
                line
            }
        }
    }

    pub fn json(&self) -> Value {
        match self {
            Receipt::NoArchive => json!({ "status": "no_archive" }),
            Receipt::NotArchived(_) => json!({ "status": "not_archived" }),
            Receipt::Found {
                session,
                agent,
                started,
                excerpt,
            } => json!({
                "status": "found", "session": session, "agent": agent,
                "started": started, "excerpt": excerpt,
            }),
        }
    }
}

/// Lowercase words of three or more letters or digits.
fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect()
}

/// The session text, title aside, sharing the most of `wanted`; the earliest
/// on a tie. `None` when no passage shares a word.
fn best_passage(conn: &Connection, session: &str, wanted: &BTreeSet<String>) -> Option<String> {
    let (mut best, mut best_score, mut after, mut read) = (None, 0, None, 0);
    while read < MAX_DOCUMENTS {
        let page = search::session_documents_page(conn, session, after, PAGE).ok()?;
        let Some(last) = page.last() else { break };
        after = Some(last.id);
        read += page.len();
        for document in page.iter().filter(|d| d.label != "title") {
            let score = words(&document.text).intersection(wanted).count();
            if score > best_score {
                (best, best_score) = (Some(document.text.clone()), score);
            }
        }
    }
    best.map(|text| excerpt(&text, wanted))
}

/// `text` on one line, from a few words before its first wanted word, cut to
/// [`EXCERPT_CHARS`].
fn excerpt(text: &str, wanted: &BTreeSet<String>) -> String {
    let flat: Vec<&str> = text.split_whitespace().collect();
    let first = flat
        .iter()
        .position(|w| words(w).iter().any(|w| wanted.contains(w)))
        .unwrap_or(0);
    let start = first.saturating_sub(4);
    let lead = if start > 0 { "..." } else { "" };
    let rest = flat[start..].join(" ").replace('"', "'");
    let cut: String = rest.chars().take(EXCERPT_CHARS).collect();
    let tail = if cut.len() < rest.len() { "..." } else { "" };
    format!("{lead}{cut}{tail}")
}

/// Epoch milliseconds as a UTC `YYYY-MM-DD`.
pub(crate) fn date(ms: i64) -> Option<String> {
    let at = OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000)).ok()?;
    Some(at.date().to_string())
}
