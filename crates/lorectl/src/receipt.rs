//! A proposal's receipt: the archived session it came from, and the passage
//! of that session it came from. That is the person's turn the proposal
//! quotes, else the person's turn that best matches it, else their first
//! turn, and only then what the agent said. Only what the person typed is
//! their turn, read as the miner reads it ([`mine::persons_words`]): the
//! words after a slash command, never a tool's output in their turn (a file
//! the agent read, a command's result) or text the harness wrote in their
//! place, such as a continued session's summary or a loaded skill.
//!
//! Only the redacted search projection is read, the same text `search` and
//! the MCP tools return, so a receipt never shows a span the secret scan
//! masked. The archive is opened read only and never created.

use std::collections::BTreeSet;
use std::path::Path;

use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::{query, search, storage};
use lore_memory::index::{related_words, stem};
use lore_memory::mine;
use rusqlite::Connection;
use serde_json::{json, Value};
use time::OffsetDateTime;

/// Characters of excerpt shown.
const EXCERPT_CHARS: usize = 160;
/// Documents read from one session, so a huge session stays bounded.
const MAX_DOCUMENTS: usize = 2_000;
const PAGE: i64 = 200;
/// A quoted span shorter than this many bytes is too common to name a turn.
const MIN_QUOTE: usize = 16;

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
        let wanted = Wanted {
            words: words(text),
            lesson: stems(text),
            quotes: quotes(text),
        };
        let excerpt = best_passage(conn, &owner.archive_id, &wanted);
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

/// The content words of `text` ([`related_words`], a line at a time, since it
/// keeps at most a dozen of a text) with their endings folded ([`stem`]).
fn stems(text: &str) -> BTreeSet<String> {
    text.lines()
        .flat_map(related_words)
        .map(|w| stem(&w))
        .collect()
}

/// What a passage should match: a proposal's title and body read three ways.
struct Wanted {
    /// Its [`words`], which place an excerpt and rank the agent's turns.
    words: BTreeSet<String>,
    /// Its [`stems`], which rank the person's turns.
    lesson: BTreeSet<String>,
    /// Its [`quotes`], which name a turn outright.
    quotes: Vec<String>,
}

/// How well one of the person's turns matches the lesson: how many of its
/// [`stems`] the lesson holds, out of how many it has.
#[derive(Clone, Copy, Default)]
struct Match {
    shared: usize,
    of: usize,
}

impl Match {
    /// Whether this turn matches better than `than`: a larger share of its
    /// words are the lesson's, or the same share and more of them. A long
    /// request shares a word or two with most lessons by chance; the
    /// correction a lesson came from is mostly about the lesson.
    fn beats(self, than: Self) -> bool {
        let (a, b) = (self.shared * than.of, than.shared * self.of);
        a > b || (a == b && self.shared > than.shared)
    }
}

/// The spans of `text` between double quotes, on one line, that are long
/// enough to name a turn. A mined proposal's body quotes each receipt.
fn quotes(text: &str) -> Vec<String> {
    let text = text.replace(['\u{201c}', '\u{201d}'], "\"");
    (text.split('"').skip(1).step_by(2))
        .map(one_line)
        .filter(|q| q.len() >= MIN_QUOTE)
        .collect()
}

/// The quote `turn` (already on one line) is, holds, or is part of.
fn quoted_in<'q>(turn: &str, quotes: &'q [String]) -> Option<&'q str> {
    quotes
        .iter()
        .find(|q| turn.contains(q.as_str()) || (turn.len() >= MIN_QUOTE && q.contains(turn)))
        .map(String::as_str)
}

/// The session's passage for a proposal, in this order: the first of the
/// person's turns one of `wanted.quotes` names; the person's turn that best
/// matches the lesson ([`Match::beats`]), the earliest on a tie, passing
/// over a go-ahead such as "Yes, merge them" ([`mine::is_approval`]); the
/// person's first turn; the agent's turn sharing the most of `wanted.words`.
/// A person's turn is the words they typed ([`mine::persons_words`]): the
/// text after a slash command, and never a tool's output, a loaded skill, or
/// other text the harness wrote in their place. `None` when the session has
/// no such turn.
fn best_passage(conn: &Connection, session: &str, wanted: &Wanted) -> Option<String> {
    let mut user: (Match, Option<String>) = (Match::default(), None);
    let mut agent: (usize, Option<String>) = (0, None);
    let mut first_user: Option<String> = None;
    let (mut after, mut read) = (None, 0);
    while read < MAX_DOCUMENTS {
        let page = search::session_documents_page(conn, session, after, PAGE).ok()?;
        let Some(last) = page.last() else { break };
        after = Some(last.id);
        read += page.len();
        // A title, a patch, or a tool's output is no one's turn.
        for document in page.iter().filter(|d| d.part.as_deref() == Some("text")) {
            if document.label != "user" {
                let score = words(&document.text).intersection(&wanted.words).count();
                if score > agent.0 {
                    agent = (score, Some(document.text.clone()));
                }
                continue;
            }
            let Some(said) = mine::persons_words(&document.text) else {
                continue;
            };
            let flat = one_line(&said);
            if let Some(quote) = quoted_in(&flat, &wanted.quotes) {
                return Some(quoted_excerpt(&flat, quote));
            }
            first_user.get_or_insert_with(|| said.clone());
            if mine::is_approval(&flat) {
                continue;
            }
            let theirs = stems(&said);
            let score = Match {
                shared: theirs.intersection(&wanted.lesson).count(),
                of: theirs.len(),
            };
            if score.shared > 0 && score.beats(user.0) {
                user = (score, Some(said));
            }
        }
    }
    (user.1.or(first_user).or(agent.1)).map(|text| excerpt(&text, &wanted.words))
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `turn` from where `quote` starts in it, or whole when it is part of
/// `quote`, cut to [`EXCERPT_CHARS`].
fn quoted_excerpt(turn: &str, quote: &str) -> String {
    match turn.find(quote) {
        Some(at) if at > 0 => clip("...", &turn[at..]),
        _ => clip("", turn),
    }
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
    clip(lead, &flat[start..].join(" "))
}

/// `lead`, then `rest` with double quotes made single, cut to
/// [`EXCERPT_CHARS`].
fn clip(lead: &str, rest: &str) -> String {
    let rest = rest.replace('"', "'");
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
    fn quotes_are_the_long_quoted_spans_and_name_a_turn_either_way() {
        let body = "Rule.\n- day agent s: \"pin the schema   version first\", after \"ok\"\n\
                    - day agent s: \u{201c}keep the changelog to one line\u{201d}\n";
        let quotes = quotes(body);
        assert_eq!(
            quotes,
            [
                "pin the schema version first",
                "keep the changelog to one line"
            ]
        );
        let turn = "No, pin the schema version first, then retry.";
        assert_eq!(
            quoted_in(turn, &quotes),
            Some("pin the schema version first")
        );
        assert_eq!(
            quoted_in("keep the changelog", &quotes),
            Some("keep the changelog to one line")
        );
        assert_eq!(quoted_in("one line", &quotes), None);
        assert_eq!(
            quoted_excerpt(turn, "pin the schema version first"),
            "...pin the schema version first, then retry."
        );
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
