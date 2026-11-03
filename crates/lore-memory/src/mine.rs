//! The corrections miner: user turns in the archive that correct the agent.
//!
//! It reads only the archive's redacted search projection, so every text it
//! returns is what `lorectl search` could show. Signals are fixed rules on the
//! words, never a model: a short user turn right after an assistant turn that
//! opens with or contains a correction ("no", "don't", "instead", "use X not
//! Y", "I told you"), or a user turn that repeats an instruction the person
//! already gave earlier in the same session. A question is neither. [`lessons`] groups candidates
//! that say the same thing across sessions. It writes nothing.

use std::collections::BTreeSet;

use rusqlite::Connection;

use crate::policy::overlap_per_mille;

/// Candidates join one lesson when their content words overlap at least this
/// much, per mille.
const LESSON_PER_MILLE: usize = 500;
/// A candidate is compared with at most this many of a lesson's oldest
/// members, so grouping stays linear in the number of candidates.
const MATCH_MEMBERS: usize = 8;
/// A lesson needs corrections from at least this many sessions.
const MIN_SESSIONS: usize = 2;

/// A correction longer than this is a new task that happens to say "no".
const MAX_CHARS: usize = 300;
/// Characters kept of the assistant turn a correction answers.
const PRECEDING_CHARS: usize = 200;
/// A repeat shares at least this much of its words, per mille, with an
/// earlier user turn, and has at least `MIN_REPEAT_WORDS` of them.
const REPEAT_PER_MILLE: usize = 700;
const MIN_REPEAT_WORDS: usize = 4;

/// The first words that open a correction.
const OPENERS: &[&str] = &[
    "no", "nope", "stop", "wrong", "don't", "dont", "never", "actually", "instead",
];
/// Openings of a question, which asks rather than corrects.
const QUESTION_OPENERS: &[&str] = &[
    "what ",
    "what's ",
    "why ",
    "how ",
    "wait so",
    "wait but",
    "can you",
    "could you",
    "is it",
];
/// Phrases that mark a correction anywhere in a short turn.
const PHRASES: &[&str] = &[
    "don't ",
    "do not ",
    " instead",
    "wrong",
    "i told you",
    "i said ",
    "not what i",
    "that's not",
    "thats not",
    "you should have",
    "stop ",
];

/// Why a user turn is a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// It corrects the assistant turn before it.
    Correction,
    /// It repeats an instruction given earlier in the session.
    Repeat,
}

/// A user turn that corrects the agent, with where it was said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The archive session id.
    pub session: String,
    /// `claude-code` or `codex`.
    pub agent: String,
    /// The repository's display name, when the archive linked one.
    pub repo: Option<String>,
    /// The repository's path on this machine, when known.
    pub repo_path: Option<String>,
    /// Epoch milliseconds.
    pub at_ms: Option<i64>,
    /// The user's turn, redacted, on one line.
    pub text: String,
    /// The assistant turn it answers, redacted, cut short.
    pub preceding: Option<String>,
    pub signal: Signal,
}

const TURNS: &str = "SELECT m.session_id, s.agent_id, m.seq, m.role, m.ts, sd.redacted_text,
        r.display_name, r.primary_path
    FROM message m
    JOIN agent_session s ON s.id = m.session_id
    JOIN message_part p ON p.message_id = m.id AND p.kind = 'text'
    JOIN search_document sd ON sd.session_id = m.session_id
        AND sd.source_kind = 'message_part' AND sd.source_id = p.id
    LEFT JOIN session_segment seg ON seg.id = m.segment_id
    LEFT JOIN repository r ON r.id = seg.repository_id
    WHERE m.role IN ('user', 'assistant') AND m.is_sidechain = 0
    ORDER BY m.session_id, m.seq, p.ordinal";

/// One message: its parts' text joined.
struct Turn {
    session: String,
    agent: String,
    seq: i64,
    user: bool,
    at_ms: Option<i64>,
    text: String,
    repo: Option<String>,
    repo_path: Option<String>,
}

/// Every candidate in the archive, by session then turn. Read only.
pub fn corrections(conn: &Connection) -> rusqlite::Result<Vec<Candidate>> {
    corrections_within(conn, &Limits::default())
}

/// The two limits a person can tune while judging what the miner finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// A user turn longer than this many characters is not a correction.
    pub max_chars: usize,
    /// Candidates join one lesson at this word overlap, per mille, or more.
    pub min_overlap_per_mille: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_chars: MAX_CHARS,
            min_overlap_per_mille: LESSON_PER_MILLE,
        }
    }
}

/// [`corrections`] with `limits.max_chars` in place of the default.
pub fn corrections_within(conn: &Connection, limits: &Limits) -> rusqlite::Result<Vec<Candidate>> {
    let mut statement = conn.prepare(TURNS)?;
    let mut rows = statement.query([])?;
    let mut turns: Vec<Turn> = Vec::new();
    while let Some(row) = rows.next()? {
        let (session, seq): (String, i64) = (row.get(0)?, row.get(2)?);
        let text: String = row.get(5)?;
        match turns.last_mut() {
            Some(last) if last.session == session && last.seq == seq => {
                last.text.push(' ');
                last.text.push_str(&text);
            }
            _ => turns.push(Turn {
                agent: row.get(1)?,
                user: row.get::<_, String>(3)? == "user",
                at_ms: row.get(4)?,
                repo: row.get(6)?,
                repo_path: row.get(7)?,
                session,
                seq,
                text,
            }),
        }
    }
    let mut found = Vec::new();
    let mut earlier: Vec<BTreeSet<String>> = Vec::new();
    for (i, turn) in turns.iter().enumerate() {
        let before = i
            .checked_sub(1)
            .map(|j| &turns[j])
            .filter(|b| b.session == turn.session);
        if before.is_none() {
            earlier.clear();
        }
        if !turn.user {
            continue;
        }
        let text = one_line(&turn.text);
        let words = content_words(&text);
        let signal = if text.starts_with('<')
            || text.chars().count() > limits.max_chars
            || is_question(&text)
        {
            None
        } else if before.is_some_and(|b| !b.user) && corrects(&text) {
            Some(Signal::Correction)
        } else if words.len() >= MIN_REPEAT_WORDS
            && earlier
                .iter()
                .any(|e| overlap_per_mille(e, &words) >= REPEAT_PER_MILLE)
        {
            Some(Signal::Repeat)
        } else {
            None
        };
        if let Some(signal) = signal {
            found.push(Candidate {
                session: turn.session.clone(),
                agent: turn.agent.clone(),
                repo: turn.repo.clone(),
                repo_path: turn.repo_path.clone(),
                at_ms: turn.at_ms,
                preceding: before
                    .filter(|b| !b.user)
                    .map(|b| one_line(&b.text).chars().take(PRECEDING_CHARS).collect()),
                text,
                signal,
            });
        }
        earlier.push(words);
    }
    Ok(found)
}

/// Whether `text` asks something: it ends with a question mark, or opens the
/// way a question does.
fn is_question(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    lower.ends_with('?') || QUESTION_OPENERS.iter().any(|q| lower.starts_with(q))
}

/// Whether `text` reads as a correction.
fn corrects(text: &str) -> bool {
    let lower = text.to_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|t| !t.is_empty())
        .collect();
    // "no worries" and "no problem" open a turn without correcting anything.
    let opener = tokens.first().is_some_and(|t| OPENERS.contains(t))
        && !matches!(tokens.get(1), Some(&("worries" | "problem")));
    let use_not = tokens
        .windows(4)
        .any(|w| w[0] == "use" && (w[2] == "not" || w[2] == "instead"));
    opener || use_not || PHRASES.iter().any(|p| format!(" {lower} ").contains(p))
}

/// Words that say what a turn is about: lowercase, three or more letters or
/// digits, without the words every correction shares.
#[must_use]
pub fn content_words(text: &str) -> BTreeSet<String> {
    const COMMON: &[&str] = &[
        "the", "and", "but", "not", "don't", "dont", "you", "your", "please", "this", "that",
        "with", "for", "use", "instead", "stop", "wrong", "told", "said", "again", "just", "nope",
        "never", "actually", "wait", "it's", "its", "are", "was", "should", "have",
    ];
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| w.chars().count() >= 3 && !COMMON.contains(w))
        .map(stem)
        .collect()
}

/// `word` without a plain `ing`, `ed`, or `s` ending, so "edit", "editing",
/// and "edited" count as one word. Short words are kept whole.
fn stem(word: &str) -> String {
    for (suffix, min) in [("ing", 6), ("ed", 5), ("s", 4)] {
        if word.len() >= min && word.ends_with(suffix) && !word.ends_with("ss") {
            return word[..word.len() - suffix.len()].to_owned();
        }
    }
    word.to_owned()
}

/// Candidates from different sessions that say the same thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lesson {
    /// Ten hex characters, the same for as long as its oldest receipt is.
    pub id: String,
    /// The newest receipt's text.
    pub text: String,
    /// Distinct sessions among the receipts.
    pub sessions: usize,
    pub latest_ms: Option<i64>,
    /// The newest candidate from each session, newest first.
    pub receipts: Vec<Candidate>,
}

/// Group `candidates` into lessons: a candidate joins the first lesson with one
/// of its first [`MATCH_MEMBERS`] members whose content words overlap its own
/// by at least half, else starts
/// one. Only lessons said in at least two sessions, at least one of them as
/// a correction, are kept, most sessions first, then the most recent.
#[must_use]
pub fn lessons(candidates: Vec<Candidate>) -> Vec<Lesson> {
    lessons_within(candidates, &Limits::default())
}

/// [`lessons`] with `limits.min_overlap_per_mille` in place of the default.
#[must_use]
pub fn lessons_within(mut candidates: Vec<Candidate>, limits: &Limits) -> Vec<Lesson> {
    candidates.sort_by(|a, b| (a.at_ms, &a.session, &a.text).cmp(&(b.at_ms, &b.session, &b.text)));
    // A resume or fork copies the conversation into a new session, user turns
    // and timestamps included. Keep one copy of each, so copies of one
    // conversation never count as two sessions. The earliest session id wins.
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| c.at_ms.is_none() || seen.insert((c.at_ms, c.text.to_lowercase())));
    let mut groups: Vec<Vec<(BTreeSet<String>, Candidate)>> = Vec::new();
    for candidate in candidates {
        let words = content_words(&candidate.text);
        if words.is_empty() {
            continue;
        }
        let joins = groups.iter().position(|group| {
            group
                .iter()
                .take(MATCH_MEMBERS)
                .any(|(other, _)| overlap_per_mille(other, &words) >= limits.min_overlap_per_mille)
        });
        match joins {
            Some(at) => groups[at].push((words, candidate)),
            None => groups.push(vec![(words, candidate)]),
        }
    }
    let mut found: Vec<Lesson> = groups.into_iter().filter_map(lesson).collect();
    found.sort_by(|a, b| (b.sessions, b.latest_ms, &a.id).cmp(&(a.sessions, a.latest_ms, &b.id)));
    found
}

/// One group, oldest first, as a lesson, or `None` below [`MIN_SESSIONS`].
fn lesson(group: Vec<(BTreeSet<String>, Candidate)>) -> Option<Lesson> {
    let oldest = group.first()?.1.clone();
    let mut receipts: Vec<Candidate> = Vec::new();
    for (_, candidate) in group.into_iter().rev() {
        if !receipts.iter().any(|r| r.session == candidate.session) {
            receipts.push(candidate);
        }
    }
    // A turn re-sent across sessions with no correction among them is a task
    // the person repeats, not a lesson.
    let corrected = receipts.iter().any(|r| r.signal == Signal::Correction);
    if receipts.len() < MIN_SESSIONS || !corrected {
        return None;
    }
    let hash = blake3::hash(format!("{}\n{}", oldest.session, oldest.text).as_bytes());
    Some(Lesson {
        id: hash.to_hex()[..10].to_owned(),
        text: receipts[0].text.clone(),
        sessions: receipts.len(),
        latest_ms: receipts[0].at_ms,
        receipts,
    })
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
