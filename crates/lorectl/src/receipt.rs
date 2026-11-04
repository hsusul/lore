//! A proposal's receipt: the archived session it came from, and the passage
//! of that session that shares the most words with what it proposes,
//! preferring what the person said over what the agent said.
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
/// on a tie. The person's own turns come first: an agent turn is quoted only
/// when no user turn shares a word. `None` when no passage shares a word.
fn best_passage(conn: &Connection, session: &str, wanted: &BTreeSet<String>) -> Option<String> {
    // Index 0 is the best user turn, 1 the best of any other role.
    let mut best: [(usize, Option<String>); 2] = [(0, None), (0, None)];
    let (mut after, mut read) = (None, 0);
    while read < MAX_DOCUMENTS {
        let page = search::session_documents_page(conn, session, after, PAGE).ok()?;
        let Some(last) = page.last() else { break };
        after = Some(last.id);
        read += page.len();
        for document in page.iter().filter(|d| d.label != "title") {
            let score = words(&document.text).intersection(wanted).count();
            let slot = &mut best[usize::from(document.label != "user")];
            if score > slot.0 {
                *slot = (score, Some(document.text.clone()));
            }
        }
    }
    let [(_, user), (_, other)] = best;
    user.or(other).map(|text| excerpt(&text, wanted))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_filters_short_tokens_and_lowercases() {
        let got = words("Run the migration with NPM on v2!");
        let expected: BTreeSet<String> = ["migration", "npm", "run", "the", "with"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(got, expected);
        assert!(!got.contains("on"));
        assert!(!got.contains("v2"));
    }

    #[test]
    fn date_formats_utc_day_from_millis() {
        assert_eq!(date(1_786_406_400_000), Some("2026-08-11".into()));
        assert_eq!(date(0), Some("1970-01-01".into()));
        assert_eq!(date(86_400_000), Some("1970-01-02".into()));
        assert_eq!(date(-86_400_000), Some("1969-12-31".into()));
    }

    #[test]
    fn excerpt_extracts_around_matching_words_and_replaces_quotes() {
        let wanted: BTreeSet<String> = ["migration"].into_iter().map(str::to_string).collect();
        let text = "First step is here and now we must run the migration before starting.";
        let got = excerpt(text, &wanted);
        assert_eq!(got, "...we must run the migration before starting.");
    }

    #[test]
    fn excerpt_replaces_double_quotes_and_caps_length() {
        let wanted: BTreeSet<String> = ["migration"].into_iter().map(str::to_string).collect();
        let text = "run the \"migration\" now";
        assert_eq!(excerpt(text, &wanted), "run the 'migration' now");

        let long_body = format!("run the migration {}", "word ".repeat(50));
        let got = excerpt(&long_body, &wanted);
        assert!(got.ends_with("..."));
        assert!(got.len() <= EXCERPT_CHARS + "...".len());
    }

    #[test]
    fn receipt_line_and_json_formatting() {
        let no_arch = Receipt::NoArchive;
        assert_eq!(
            no_arch.line(),
            "    receipt: no archive yet; `lorectl scan` builds one"
        );
        assert_eq!(no_arch.json(), json!({ "status": "no_archive" }));

        let not_found = Receipt::NotArchived("s-missing".into());
        assert_eq!(
            not_found.line(),
            "    receipt: session s-missing is not in the archive"
        );
        assert_eq!(not_found.json(), json!({ "status": "not_archived" }));

        let found = Receipt::Found {
            session: "s-123".into(),
            agent: "codex".into(),
            started: Some("2026-08-11".into()),
            excerpt: Some("run migration".into()),
        };
        assert_eq!(
            found.line(),
            "    from codex session s-123, 2026-08-11: \"run migration\""
        );
        assert_eq!(
            found.json(),
            json!({
                "status": "found",
                "session": "s-123",
                "agent": "codex",
                "started": "2026-08-11",
                "excerpt": "run migration",
            })
        );

        let minimal_found = Receipt::Found {
            session: "s-456".into(),
            agent: "claude-code".into(),
            started: None,
            excerpt: None,
        };
        assert_eq!(minimal_found.line(), "    from claude-code session s-456");
        assert_eq!(
            minimal_found.json(),
            json!({
                "status": "found",
                "session": "s-456",
                "agent": "claude-code",
                "started": Value::Null,
                "excerpt": Value::Null,
            })
        );
    }
}
