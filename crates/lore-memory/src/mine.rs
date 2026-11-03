//! The corrections miner: user turns in the archive that correct the agent.
//!
//! It reads only the archive's redacted search projection, so every text it
//! returns is what `lorectl search` could show. Signals are fixed rules on the
//! words, never a model: a short user turn right after an assistant turn that
//! opens with or contains a correction ("no", "don't", "instead", "use X not
//! Y", "I told you"), or a user turn that repeats an instruction the person
//! already gave earlier in the same session. A question is neither, and nor
//! is a turn that starts by invoking a skill or a slash command. [`lessons`] groups candidates
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

/// Phrases that state a standing rule wherever they appear.
const RULE_PHRASES: &[&str] = &["make sure", "please always", "from now on", "going forward"];

/// Words that make a turn a rule for later work.
const RULE_WORDS: &[&str] = &[
    "always",
    "never",
    "don't",
    "dont",
    "stop",
    "convention",
    "conventions",
    "style",
];
/// Tools and commands a rule can name. Names that are everyday words, like
/// make, just, and go, are left out.
const TOOLS: &[&str] = &[
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "node",
    "npx",
    "cargo",
    "rustc",
    "rustfmt",
    "clippy",
    "git",
    "gh",
    "uv",
    "pip",
    "python",
    "python3",
    "pytest",
    "poetry",
    "docker",
    "jest",
    "vitest",
    "eslint",
    "prettier",
    "tsc",
    "brew",
    "kubectl",
    "terraform",
];

/// Why a user turn is a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// It corrects the assistant turn before it.
    Correction,
    /// It repeats an instruction given earlier in the session.
    Repeat,
    /// It states a standing rule: "make sure", "please always", "from now on",
    /// or "going forward".
    Rule,
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
    let turns = read_turns(conn)?;
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
            || is_invocation(&text)
        {
            None
        } else if before.is_some_and(|b| !b.user) && corrects(&text) {
            Some(Signal::Correction)
        } else if has_rule_phrase(&text) {
            Some(Signal::Rule)
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

/// Every user and assistant message in the archive, its parts' text joined,
/// in session then turn order.
fn read_turns(conn: &Connection) -> rusqlite::Result<Vec<Turn>> {
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
    Ok(turns)
}

/// Whether `text` starts by invoking a skill, `[$name](path) ...`, or a slash
/// command, `/name ...`. A path such as `/home/me/x` is not a command.
fn is_invocation(text: &str) -> bool {
    let first = text.split_whitespace().next().unwrap_or_default();
    text.starts_with("[$")
        || (first.len() > 1 && first.starts_with('/') && !first[1..].contains('/'))
}

/// Whether `text` asks something: it ends with a question mark, or opens the
/// way a question does.
fn is_question(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    lower.ends_with('?') || QUESTION_OPENERS.iter().any(|q| lower.starts_with(q))
}

/// Whether `text` uses a phrase that states a standing rule.
fn has_rule_phrase(text: &str) -> bool {
    let lower = text.to_lowercase();
    RULE_PHRASES.iter().any(|p| lower.contains(p))
}

/// Whether `text` reads as a correction.
fn corrects(text: &str) -> bool {
    let lower = text.to_lowercase();
    let tokens = tokens(&lower);
    // "no worries" and "no problem" open a turn without correcting anything.
    let opener = tokens.first().is_some_and(|t| OPENERS.contains(t))
        && !matches!(tokens.get(1), Some(&("worries" | "problem")));
    let use_not = use_x_not_y(&tokens);
    opener || use_not || PHRASES.iter().any(|p| format!(" {lower} ").contains(p))
}

/// Lowercase words, keeping apostrophes, so "don't" stays one word.
fn tokens(lower: &str) -> Vec<&str> {
    lower
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|t| !t.is_empty())
        .collect()
}

/// "use X not Y" or "use X instead".
fn use_x_not_y(tokens: &[&str]) -> bool {
    tokens
        .windows(3)
        .any(|w| w[0] == "use" && (w[2] == "not" || w[2] == "instead"))
}

/// Whether `text` states a rule that would apply to later work, not only to
/// the task at hand: it names a tool, command, flag, file, or path, says
/// always, never, don't, do not, or stop, speaks of a convention or style,
/// or says use X not Y. A redirect like "give me the short one instead"
/// does none of these.
#[must_use]
pub fn states_rule(text: &str) -> bool {
    let lower = text.to_lowercase();
    let words = tokens(&lower);
    let names_a_thing = lower.split_whitespace().any(|raw| {
        let word = raw.trim_matches(|c: char| ",.;:!?()'\"".contains(c));
        word.contains('`')
            || word.starts_with("--")
            || (word.contains('/') && word.chars().any(char::is_alphanumeric))
            || word.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty()
                    && (1..=5).contains(&ext.len())
                    && ext.chars().all(|c| c.is_ascii_alphanumeric())
            })
    });
    names_a_thing
        || has_rule_phrase(&lower)
        || lower.contains("do not ")
        || use_x_not_y(&words)
        || words
            .iter()
            .any(|w| RULE_WORDS.contains(w) || TOOLS.contains(w))
}

/// Phrases that make a turn read as a standing rule. "make sure" counts for
/// less when the same turn holds a task verb (see [`rule_score`]).
const STANDING: &[&str] = &["from now on", "going forward", "always", "never"];
/// First words of a one-off task order.
const TASK_VERBS: &[&str] = &[
    "push",
    "open",
    "merge",
    "ship",
    "deploy",
    "rebase",
    "commit",
    "create",
    "fix",
    "close",
    "review",
    "revert",
    "release",
    "checkout",
    "cherry-pick",
    "tag",
];

/// How much `text` reads like a standing rule rather than a one-off task
/// order. Fixed and explainable: +2 for a standing phrase ("from now on",
/// "always", "never", "going forward"), or for "make sure" alone, which earns
/// only +1 when the turn also holds a task verb; +1 when it names no specific
/// artifact; -3 for each PR or issue number (`#478`, `issue 12`), each
/// branch-shaped token (a slash and no file extension), each URL, and each
/// span the secret scan masked; and -2 when it opens with a task verb
/// ("push", "merge", "fix", ...) and names an artifact.
#[must_use]
pub fn rule_score(text: &str) -> i32 {
    let lower = text.to_lowercase();
    let tokens: Vec<&str> = lower
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| ",.;:!?()\"'`".contains(c)))
        .filter(|t| !t.is_empty())
        .collect();
    let digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    let numbers = tokens
        .iter()
        .enumerate()
        .filter(|(i, t)| {
            t.strip_prefix('#').is_some_and(digits)
                || (matches!(**t, "issue" | "issues" | "pr" | "pull")
                    && tokens.get(i + 1).is_some_and(|n| digits(n)))
        })
        .count();
    let has_extension = |t: &str| {
        t.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && (1..=5).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
    };
    let branches = tokens
        .iter()
        .filter(|t| {
            let branch = t.contains('/')
                && !t.starts_with(['/', '~', '.'])
                && !has_extension(t)
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_/".contains(c));
            // A URL and a span the secret scan masked name one artifact as
            // surely as a branch does.
            branch || t.contains("://") || t.contains("«redacted:")
        })
        .count();
    let paths = tokens
        .iter()
        .filter(|t| has_extension(t) && !t.contains("://") && !t.contains("«redacted:"))
        .count();
    let artifacts = numbers + branches + paths;
    let mut score = 0;
    let task_verb = tokens.iter().any(|t| TASK_VERBS.contains(t));
    if STANDING.iter().any(|p| lower.contains(p)) {
        score += 2;
    } else if lower.contains("make sure") {
        score += if task_verb { 1 } else { 2 };
    }
    if artifacts == 0 {
        score += 1;
    }
    score -= 3 * i32::try_from(numbers + branches).unwrap_or(i32::MAX / 3);
    if artifacts > 0 && tokens.first().is_some_and(|t| TASK_VERBS.contains(t)) {
        score -= 2;
    }
    score
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

/// Candidates that say the same thing, from one conversation or several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lesson {
    /// Ten hex characters, the same for as long as its oldest receipt is.
    pub id: String,
    /// The newest receipt's text that states a rule.
    pub text: String,
    /// Distinct conversations among the receipts. A resume or fork copy of a
    /// conversation is not counted again.
    pub conversations: usize,
    /// The best [`rule_score`] among the receipts.
    pub score: i32,
    pub latest_ms: Option<i64>,
    /// The newest candidate from each session, newest first.
    pub receipts: Vec<Candidate>,
    /// Later user turns about the same command in the same repository, newest
    /// first, set by [`find_conflicts`]. Candidates to judge, not verdicts.
    pub conflicts: Vec<Candidate>,
}

/// Group `candidates` into lessons: a candidate joins the first lesson with one
/// of its first [`MATCH_MEMBERS`] members whose content words overlap its own
/// by at least half, else starts one. A lesson is kept when one of its
/// receipts is a correction or a rule and one states a rule. Lessons that
/// read most like a standing rule ([`rule_score`]) come first, then those
/// said in more conversations, then the most recent.
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
    found.sort_by(|a, b| {
        (b.score, b.conversations, b.latest_ms, &a.id).cmp(&(
            a.score,
            a.conversations,
            a.latest_ms,
            &b.id,
        ))
    });
    found
}

/// One group, oldest first, as a lesson, or `None` when it holds no correction
/// or states no rule.
fn lesson(group: Vec<(BTreeSet<String>, Candidate)>) -> Option<Lesson> {
    let oldest = group.first()?.1.clone();
    let mut receipts: Vec<Candidate> = Vec::new();
    for (_, candidate) in group.into_iter().rev() {
        if !receipts.iter().any(|r| r.session == candidate.session) {
            receipts.push(candidate);
        }
    }
    // A turn re-sent across sessions with no correction among them is a task
    // the person repeats, not a lesson; one that never states a rule is a
    // redirect of the task at hand.
    let corrected =
        (receipts.iter()).any(|r| matches!(r.signal, Signal::Correction | Signal::Rule));
    let rule = receipts.iter().find(|r| states_rule(&r.text))?.text.clone();
    if !corrected {
        return None;
    }
    let hash = blake3::hash(format!("{}\n{}", oldest.session, oldest.text).as_bytes());
    Some(Lesson {
        id: hash.to_hex()[..10].to_owned(),
        text: rule,
        conversations: receipts.len(),
        score: receipts
            .iter()
            .map(|r| rule_score(&r.text))
            .max()
            .unwrap_or(0),
        latest_ms: receipts[0].at_ms,
        receipts,
        conflicts: Vec::new(),
    })
}

/// Command families a lesson can be about, each with the words that name it.
const FAMILIES: &[(&str, &[&str])] = &[
    ("pr", &["pr", "prs", "pull request", "pull requests"]),
    ("push", &["push"]),
    ("commit", &["commit", "commits"]),
    ("branch", &["branch", "branches"]),
    ("merge", &["merge"]),
    ("rebase", &["rebase"]),
    ("tests", &["test", "tests"]),
    ("mocks", &["mock", "mocks", "mocking"]),
    ("docs", &["docs", "documentation"]),
    ("permission", &["permission", "permissions"]),
];
/// The clauses of `text`, lowercase, each padded with spaces for whole-word
/// matching. Clauses end at punctuation, "just", "but", and "instead".
fn clauses(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    lower
        .split(|c: char| ",.;!?".contains(c))
        .flat_map(|c| c.split(" just "))
        .flat_map(|c| c.split(" but "))
        .flat_map(|c| c.split(" instead "))
        .map(|c| format!(" {} ", tokens(c).join(" ")))
        .collect()
}

/// Whether `text` names `family` by one of its words.
fn names_family(text: &str, words: &[&str]) -> bool {
    clauses(text)
        .iter()
        .any(|clause| words.iter().any(|w| clause.contains(&format!(" {w} "))))
}

/// The words of the first command family `text` names.
fn family_of(text: &str) -> Option<&'static [&'static str]> {
    let clauses = clauses(text);
    clauses.iter().find_map(|clause| {
        let (_, words) = FAMILIES
            .iter()
            .find(|(_, words)| words.iter().any(|w| clause.contains(&format!(" {w} "))))?;
        Some(*words)
    })
}

/// Turns listed beside one lesson, at most.
const MAX_CONFLICTS: usize = 3;

/// List, beside each lesson, the later user turns that may conflict with it:
/// turns in the same repository, newer than its newest receipt, from a
/// session none of its receipts came from, that name the same command family
/// (PR, push, commit, branch, merge, rebase, tests, mocks, docs, permission).
/// At most [`MAX_CONFLICTS`], newest first. Lore does not decide whether a
/// turn overrides the lesson; that takes reading it. Read only.
pub fn find_conflicts(conn: &Connection, lessons: &mut [Lesson]) -> rusqlite::Result<()> {
    let turns = read_turns(conn)?;
    for lesson in lessons.iter_mut() {
        let (Some(words), Some(after)) = (family_of(&lesson.text), lesson.latest_ms) else {
            continue;
        };
        let repo = lesson.receipts.first().and_then(|r| r.repo.clone());
        let mut later: Vec<&Turn> = turns
            .iter()
            .filter(|t| t.user && t.at_ms.is_some_and(|at| at > after) && t.repo == repo)
            .filter(|t| !lesson.receipts.iter().any(|r| r.session == t.session))
            .filter(|t| t.text.chars().count() <= MAX_CHARS && names_family(&t.text, words))
            .collect();
        later.sort_by_key(|t| std::cmp::Reverse(t.at_ms));
        lesson.conflicts = later
            .into_iter()
            .take(MAX_CONFLICTS)
            .map(|t| Candidate {
                session: t.session.clone(),
                agent: t.agent.clone(),
                repo: t.repo.clone(),
                repo_path: t.repo_path.clone(),
                at_ms: t.at_ms,
                text: one_line(&t.text),
                preceding: None,
                signal: Signal::Correction,
            })
            .collect();
    }
    Ok(())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
