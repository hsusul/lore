//! The corrections miner: user turns in the archive that correct the agent.
//!
//! It reads only the archive's redacted search projection, so every text it
//! returns is what `lorectl search` could show. Signals are fixed rules on the
//! words, never a model: a short user turn right after an assistant turn that
//! opens with or contains a correction ("no", "don't", "instead", "use X not
//! Y", "I told you"), or a user turn that repeats an instruction the person
//! already gave earlier in the same session. A question is neither unless it
//! insists ("why do you always ...", "... i dont want that?"), and nor is a
//! turn that starts by invoking a skill or a slash command, though the words
//! typed after a Claude Code command are read as a turn of their own.
//! [`lessons`] groups candidates
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
/// Characters kept of the assistant turn a correction answers: enough for
/// whoever writes the lesson to see what the agent had done.
const PRECEDING_CHARS: usize = 600;
/// Characters kept of the assistant turn that answers the correction, which
/// often says in the agent's words what it got wrong.
const FOLLOWING_CHARS: usize = 400;
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
    "dont ",
    "do not ",
    "didn't ",
    "didnt ",
    "shouldn't ",
    "shouldnt ",
    " never ",
    " instead",
    "wrong",
    "i told you",
    "i said ",
    "not what i",
    "that's not",
    "thats not",
    "you should have",
    "are you sure",
    "stop ",
];

/// Phrases that say the agent's output is too much of something: a correction
/// and a rule of style at once. "to long" is how it is often typed.
const EXCESS: &[&str] = &[
    "too long",
    "too short",
    "too many",
    "too much",
    "too verbose",
    "way too ",
    " to long",
    " to many ",
    " to much ",
];
/// Phrases that state what the person wants instead: a correction and a rule
/// at once.
const PREFERENCES: &[&str] = &["i only want", "i prefer", "i'd rather", " id rather"];

/// Phrases that state a standing rule wherever they appear.
const RULE_PHRASES: &[&str] = &[
    "make sure",
    "please always",
    "from now on",
    "going forward",
    "every time",
    "each time",
    "whenever",
];

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
    /// "going forward", "every time", "each time", or "whenever".
    Rule,
    /// An ordinary reply to the agent that no rule flagged, offered for an
    /// agent to judge. See [`replies_in`].
    Reply,
}

impl Signal {
    /// How `lorectl mem mine --json` and `mine_candidates` name it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Signal::Correction => "correction",
            Signal::Repeat => "repeat",
            Signal::Rule => "rule",
            Signal::Reply => "reply",
        }
    }
}

/// How far back a lessons run offers ordinary replies: two days, so a run a
/// day apart reads each reply once and a missed day is still caught.
pub const REPLY_WINDOW_MS: i64 = 48 * 60 * 60 * 1000;

/// How a continued Claude Code session opens: a summary the harness wrote.
const CONTINUED: &str = "This session is being continued";

/// How user turns open that the harness wrote, not the person: an interrupt
/// notice, the text of a skill being loaded, a message relayed from another
/// agent session, and the notes the host app writes around a usage limit:
/// one asking to carry on once it has reset, written in the first person,
/// a bracketed grace-period notice, and a bracketed notice that the earlier
/// ones no longer apply. A lessons run would read each one and drop it.
const NOT_THE_PERSON: &[&str] = &[
    "[Request interrupted",
    "Base directory for this skill",
    "Another Claude session sent a message",
    "I hit my usage limit while you were working",
    "[Usage limit reached",
    "[Earlier usage-limit notes",
];

/// Whether a user turn is text the harness wrote, not the person: a
/// continued session's summary, a loaded or re-invoked skill, an interrupt
/// notice, a relayed message, an image with no words, or a `<...>` block
/// such as pasted content or a system reminder.
#[must_use]
pub fn is_harness_text(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with('<')
        || text.starts_with(REINVOKED)
        || text.starts_with(CONTINUED)
        || NOT_THE_PERSON.iter().any(|p| text.starts_with(p))
        || is_bare_image(text)
}

/// The words the person wrote in `raw`, a user turn as the agent recorded
/// or sent it, read as the miner reads a turn: the words after a slash
/// command, or the text without harness blocks such as `<system-reminder>`
/// or `<task-notification>`. `None` when nothing is left or what is left is
/// harness text ([`is_harness_text`]).
#[must_use]
pub fn persons_words(raw: &str) -> Option<String> {
    let text = command_args(raw).unwrap_or_else(|| strip_harness(raw));
    let text = text.trim();
    (!text.is_empty() && !is_harness_text(text)).then(|| text.to_owned())
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
    /// The assistant turn that answers it, redacted, cut short.
    pub following: Option<String>,
    pub signal: Signal,
}

/// Every user and assistant text part, in session then turn order, given a
/// filter on `m` to put in the `WHERE` clause.
macro_rules! turns_query {
    ($filter:literal) => {
        concat!(
            "SELECT m.session_id, s.agent_id, m.seq, m.role, m.ts, sd.redacted_text,
        r.display_name, r.primary_path
    FROM message m
    CROSS JOIN agent_session s ON s.id = m.session_id
    CROSS JOIN message_part p ON p.message_id = m.id AND p.kind = 'text'
    CROSS JOIN search_document sd ON sd.session_id = m.session_id
        AND sd.source_kind = 'message_part' AND sd.source_id = p.id
    LEFT JOIN session_segment seg ON seg.id = m.segment_id
    LEFT JOIN repository r ON r.id = seg.repository_id
    WHERE m.role IN ('user', 'assistant') AND m.is_sidechain = 0",
            $filter,
            "
    ORDER BY m.session_id, m.seq, p.ordinal"
        )
    };
}

const TURNS: &str = turns_query!("");
/// [`TURNS`] for the sessions with a message at or after `?1`.
const TURNS_SINCE: &str =
    turns_query!(" AND m.session_id IN (SELECT session_id FROM message WHERE ts >= ?1)");

/// One message: its parts' text joined.
pub(crate) struct Turn {
    pub(crate) session: String,
    agent: String,
    pub(crate) seq: i64,
    pub(crate) user: bool,
    pub(crate) at_ms: Option<i64>,
    text: String,
    repo: Option<String>,
    repo_path: Option<String>,
}

/// A user turn that invoked a skill or slash command by name: `/name` or
/// `[$name](path)`, read before harness blocks are stripped, since a slash
/// command's turn can be nothing but them.
pub(crate) struct Invoked {
    pub(crate) session: String,
    pub(crate) seq: i64,
    pub(crate) at_ms: Option<i64>,
    pub(crate) name: String,
    /// Whether the person typed words after the command, which the turn's
    /// text then holds: see [`command_args`].
    pub(crate) typed: bool,
}

/// Every user and assistant turn in the archive, read once so the miner's
/// passes can share it: reading them is most of what a mining call costs.
pub struct Turns(pub(crate) Vec<Turn>, pub(crate) Vec<Invoked>);

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
    Ok(corrections_in(&read_turns(conn)?, limits))
}

/// [`corrections_within`] over turns already read.
#[must_use]
pub fn corrections_in(turns: &Turns, limits: &Limits) -> Vec<Candidate> {
    corrections_at(turns, limits)
        .into_iter()
        .map(|(_, c)| c)
        .collect()
}

/// The user turns that invoked a skill or slash command with nothing typed
/// after it, by session and seq.
fn bare_commands(turns: &Turns) -> BTreeSet<(&str, i64)> {
    (turns.1.iter())
        .filter(|i| !i.typed)
        .map(|i| (i.session.as_str(), i.seq))
        .collect()
}

/// The agent turn the user turn at `i` answers: the turn before it, or,
/// when the person sent several messages in a row while the agent worked,
/// the agent turn before that run. Only the person's own words make a run:
/// harness text, a bare command, or a message relayed from another agent
/// ends it, and so does the start of the session.
fn agent_before<'t>(turns: &'t [Turn], bare: &BTreeSet<(&str, i64)>, i: usize) -> Option<&'t Turn> {
    let session = &turns.get(i)?.session;
    for before in turns[..i].iter().rev() {
        if &before.session != session {
            return None;
        }
        if !before.user {
            return Some(before);
        }
        let text = one_line(&before.text);
        if is_harness_text(&text)
            || is_invocation(&text)
            || bare.contains(&(before.session.as_str(), before.seq))
        {
            return None;
        }
    }
    None
}

/// The agent turn that answers the user turn at `i`: the first agent turn
/// after it in the same session, past any more of the person's own words
/// sent before the agent replied. Harness text, a bare command, or an
/// invocation ends the search, as in [`agent_before`], since the agent's next
/// words then answer that and not this turn; so does the end of the session.
/// Only turns with text are read, so an agent turn that was nothing but tool
/// calls is passed over, here and in [`agent_before`].
fn agent_after<'t>(turns: &'t [Turn], bare: &BTreeSet<(&str, i64)>, i: usize) -> Option<&'t Turn> {
    let session = &turns.get(i)?.session;
    for after in turns.get(i + 1..)? {
        if &after.session != session {
            return None;
        }
        if !after.user {
            return Some(after);
        }
        let text = one_line(&after.text);
        if is_harness_text(&text)
            || is_invocation(&text)
            || bare.contains(&(after.session.as_str(), after.seq))
        {
            return None;
        }
    }
    None
}

/// A reply longer than this many words is never read as a go-ahead.
const MAX_APPROVAL_WORDS: usize = 6;

/// Words a go-ahead opens with: the person agrees, or tells the agent to
/// carry on with what it proposed, or to leave it for now.
const APPROVALS: &[&str] = &[
    "yes",
    "yeah",
    "yep",
    "yup",
    "ok",
    "okay",
    "sure",
    "go ahead",
    "go for it",
    "do it",
    "push",
    "merge",
    "commit",
    "continue",
    "proceed",
    "try again",
    "not now",
    "sounds good",
    "lgtm",
    "thanks",
    "thank you",
];

/// Words that, after a go-ahead's opening, make it more than one: the person
/// qualifies it ("yes but", "merge, not squash"), or asks for something else
/// first.
const HEDGES: &[&str] = &[
    "but",
    "not",
    "no",
    "don't",
    "dont",
    "never",
    "instead",
    "without",
    "except",
    "only",
    "rather",
    "wait",
    "actually",
    "first",
    "also",
    "why",
    "with",
    "using",
    "back",
    "other",
    "different",
    "revert",
    "undo",
];

/// Whether a reply is only a go-ahead, such as "Yes, merge them", "Push and
/// reply", "Try again", or "Not now": at most [`MAX_APPROVAL_WORDS`] words,
/// opening with one of [`APPROVALS`], with no question, no [`HEDGES`] after
/// the opening, and nothing the rule miner reads as a correction or a rule.
/// "yes but don't push" is not one. A go-ahead corrects nothing, so a lessons
/// run is not paid to read it.
#[must_use]
pub fn is_approval(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let words = tokens(&lower);
    if words.is_empty()
        || words.len() > MAX_APPROVAL_WORDS
        || lower.contains('?')
        || corrects(&lower)
        || has_rule_phrase(&lower)
    {
        return false;
    }
    if words == ["go"] {
        return true;
    }
    let Some(rest) = APPROVALS.iter().find_map(|approval| {
        let opening: Vec<&str> = approval.split(' ').collect();
        words.starts_with(&opening).then(|| &words[opening.len()..])
    }) else {
        return false;
    };
    !rest.iter().any(|w| HEDGES.contains(w))
}

/// [`corrections_in`], each with the index of its turn in `turns.0`.
pub(crate) fn corrections_at(turns: &Turns, limits: &Limits) -> Vec<(usize, Candidate)> {
    let bare = bare_commands(turns);
    let turns = &turns.0;
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
        let agent = agent_before(turns, &bare, i);
        let text = one_line(&turn.text);
        let words = content_words(&text);
        let question = is_question(&text);
        let signal = if is_harness_text(&text)
            || text.chars().count() > limits.max_chars
            || (question && !insists(&text))
            || is_invocation(&text)
        {
            None
        } else if agent.is_some() && (question || corrects(&text)) {
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
            found.push((
                i,
                Candidate {
                    session: turn.session.clone(),
                    agent: turn.agent.clone(),
                    repo: turn.repo.clone(),
                    repo_path: turn.repo_path.clone(),
                    at_ms: turn.at_ms,
                    preceding: agent
                        .map(|b| one_line(&b.text).chars().take(PRECEDING_CHARS).collect()),
                    following: agent_after(turns, &bare, i)
                        .map(|a| one_line(&a.text).chars().take(FOLLOWING_CHARS).collect()),
                    text,
                    signal,
                },
            ));
        }
        earlier.push(words);
    }
    found
}

/// A turn that is only image placeholders, such as `[Image #1] [Image #2]`.
fn is_bare_image(text: &str) -> bool {
    let mut rest = text.trim();
    if rest.is_empty() {
        return false;
    }
    while let Some(after) = rest.strip_prefix("[Image") {
        let Some(end) = after.find(']') else {
            return false;
        };
        rest = after[end + 1..].trim_start();
    }
    rest.is_empty()
}

/// The person's ordinary replies to the agent at or after `since_ms` that
/// [`corrections_at`] did not pick, as [`Signal::Reply`] candidates, oldest
/// first. A reply is a user turn right after an assistant turn in the same
/// session, or later in a run of the person's turns right after one (see
/// [`agent_before`]), that does not invoke a skill or slash command, is not a
/// continued session's summary or other text the harness wrote (an
/// interrupt notice, a bare image, a loaded skill), does not open with
/// leftover markup, and is not only a go-ahead ([`is_approval`]). Its
/// `preceding` is the last agent text before it and its `following` the
/// first agent text after it ([`agent_after`]), both from its own session.
/// Its text is kept whole; whoever shows it cuts it short. A resume or fork
/// copy of a reply is kept once, the earliest session's, as in [`lessons`].
/// A turn flagged as a correction whose group [`lessons`] drops, because no
/// turn in it states a rule ([`states_rule`]), is offered here instead, so
/// no correction reaches neither a lesson nor a lessons run.
/// Most replies correct nothing: they are for an agent to judge, not rules.
#[must_use]
pub fn replies_in(turns: &Turns, limits: &Limits, since_ms: i64) -> Vec<Candidate> {
    let picked = in_lessons(turns, limits);
    let invoked = bare_commands(turns);
    let turns = &turns.0;
    let mut found: Vec<Candidate> = Vec::new();
    for (i, turn) in turns.iter().enumerate() {
        if !turn.user
            || picked.contains(&i)
            || turn.at_ms.unwrap_or(i64::MIN) < since_ms
            || invoked.contains(&(turn.session.as_str(), turn.seq))
        {
            continue;
        }
        let Some(before) = agent_before(turns, &invoked, i) else {
            continue;
        };
        let text = one_line(&turn.text);
        if text.is_empty() || is_harness_text(&text) || is_invocation(&text) || is_approval(&text) {
            continue;
        }
        found.push(Candidate {
            session: turn.session.clone(),
            agent: turn.agent.clone(),
            repo: turn.repo.clone(),
            repo_path: turn.repo_path.clone(),
            at_ms: turn.at_ms,
            preceding: Some(
                one_line(&before.text)
                    .chars()
                    .take(PRECEDING_CHARS)
                    .collect(),
            ),
            following: agent_after(turns, &invoked, i)
                .map(|a| one_line(&a.text).chars().take(FOLLOWING_CHARS).collect()),
            text,
            signal: Signal::Reply,
        });
    }
    found.sort_by(|a, b| (a.at_ms, &a.session, &a.text).cmp(&(b.at_ms, &b.session, &b.text)));
    let mut seen = std::collections::HashSet::new();
    found.retain(|c| c.at_ms.is_none() || seen.insert((c.at_ms, c.text.to_lowercase())));
    found
}

/// The indexes in `turns.0` of the turns [`corrections_at`] flags, less the
/// corrections in a group [`lesson`] drops, which fall through to
/// [`replies_in`]. A repeat or a rule in a dropped group stays out of both.
fn in_lessons(turns: &Turns, limits: &Limits) -> BTreeSet<usize> {
    let flagged = corrections_at(turns, limits);
    let mut picked: BTreeSet<usize> = flagged.iter().map(|(i, _)| *i).collect();
    for group in groups(flagged, limits) {
        let (at, members): (Vec<usize>, Vec<Candidate>) = group.into_iter().unzip();
        let signals: Vec<Signal> = members.iter().map(|c| c.signal).collect();
        if lesson(members).is_none() {
            for (i, signal) in at.into_iter().zip(signals) {
                if signal == Signal::Correction {
                    picked.remove(&i);
                }
            }
        }
    }
    picked
}

/// Each reply as a lesson of its own, newest first: replies are never grouped
/// with each other or with rule-mined lessons, and no [`rule_score`] hides
/// them.
#[must_use]
pub fn reply_lessons(replies: Vec<Candidate>) -> Vec<Lesson> {
    let mut found: Vec<Lesson> = replies
        .into_iter()
        .map(|reply| {
            let hash = blake3::hash(format!("{}\n{}", reply.session, reply.text).as_bytes());
            Lesson {
                id: hash.to_hex()[..10].to_owned(),
                text: reply.text.clone(),
                conversations: 1,
                score: 0,
                latest_ms: reply.at_ms,
                receipts: vec![reply],
                conflicts: Vec::new(),
            }
        })
        .collect();
    found.reverse();
    found
}

/// Every user and assistant message in the archive, its parts' text joined,
/// in session then turn order.
pub fn read_turns(conn: &Connection) -> rusqlite::Result<Turns> {
    collect_turns(&mut conn.prepare(TURNS)?, [])
}

/// [`read_turns`] for only the sessions with a message at or after
/// `since_ms`, each whole, so a window costs what its sessions cost and not
/// what the archive costs.
pub fn read_turns_since(conn: &Connection, since_ms: i64) -> rusqlite::Result<Turns> {
    collect_turns(&mut conn.prepare(TURNS_SINCE)?, [since_ms])
}

fn collect_turns(
    statement: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> rusqlite::Result<Turns> {
    let mut rows = statement.query(params)?;
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
                user: row.get_ref(3)?.as_str()? == "user",
                at_ms: row.get(4)?,
                repo: row.get(6)?,
                repo_path: row.get(7)?,
                session,
                seq,
                text,
            }),
        }
    }
    let mut invoked = Vec::new();
    for turn in &mut turns {
        let args = turn.user.then(|| command_args(&turn.text)).flatten();
        if let Some(name) = turn.user.then(|| invoked_name(&turn.text)).flatten() {
            invoked.push(Invoked {
                session: turn.session.clone(),
                seq: turn.seq,
                at_ms: turn.at_ms,
                name,
                typed: args.is_some(),
            });
        }
        turn.text = args.unwrap_or_else(|| strip_harness(&turn.text));
    }
    turns.retain(|t| !t.text.trim().is_empty());
    Ok(Turns(turns, invoked))
}

/// How Claude Code wraps text the person pasted into a turn:
/// `<pasted_content id="...">`, closed by `</pasted_content id="...">`.
const PASTED: &str = "pasted_content";

/// `text` without the blocks an agent harness injects into a turn, such as
/// `<command-name>`, `<command-args>`, `<system-reminder>`, and
/// `<local-command-stdout>`: any `<name>...</name>` whose name is lowercase
/// words joined by hyphens. Other markup, like `<div>`, stays. A pasted
/// block ([`PASTED`]) goes too, so the words the person typed before or
/// after it are read on their own; a turn that is only pasted text is kept
/// whole, as harness text, since nothing in it was typed.
fn strip_harness(text: &str) -> String {
    let stripped = strip_blocks(text, true);
    if stripped.trim().is_empty() && text.contains(PASTED) {
        strip_blocks(text, false)
    } else {
        stripped
    }
}

/// [`strip_harness`], with pasted blocks stripped only when `pastes`.
fn strip_blocks(text: &str, pastes: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let name_len = after
            .bytes()
            .take_while(|b| b.is_ascii_lowercase() || *b == b'-' || *b == b'_')
            .count();
        let name = &after[..name_len];
        let end = if name.contains('-') && after[name_len..].starts_with('>') {
            find_close_tag(&after[name_len + 1..], name)
                .map(|pos| (name_len + 1 + pos, name.len() + 3))
        } else if pastes && name == PASTED {
            pasted_end(&after[name_len..]).map(|end| (name_len + end, 0))
        } else {
            None
        };
        match end {
            Some((end_pos, close_len)) => {
                out.push_str(&rest[..open]);
                out.push(' ');
                rest = &after[end_pos + close_len..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The words the person typed after a slash command or skill in Claude Code,
/// which stores `/name some words` as a `<command-message>` echo, a
/// `<command-name>`, and the words in `<command-args>`. Only those words are
/// the person's: the echo, the name, and anything the harness put in the
/// same turn after them, such as a loaded skill's text, are not. `None` when
/// the turn is not a command or the command had no words after it.
fn command_args(raw: &str) -> Option<String> {
    if !raw.contains("<command-name>") {
        return None;
    }
    let (_, after) = raw.split_once("<command-args>")?;
    let (args, _) = after.split_once("</command-args>")?;
    let args = args.trim();
    (!args.is_empty()).then(|| args.to_owned())
}

/// Where a pasted block ends in `rest`, the text after its tag's name: past
/// the `>` of its closing tag, `</pasted_content>` or one with attributes
/// such as `</pasted_content id="...">`. `None` when the opening tag or the
/// block is not closed.
fn pasted_end(rest: &str) -> Option<usize> {
    if !rest.starts_with(['>', ' ']) {
        return None;
    }
    let body = rest.find('>')? + 1;
    let close = format!("</{PASTED}");
    let mut from = body;
    loop {
        let at = from + rest[from..].find(&close)?;
        let tail = &rest[at + close.len()..];
        if tail.starts_with(['>', ' ']) {
            return Some(at + close.len() + tail.find('>')? + 1);
        }
        from = at + close.len();
    }
}

fn find_close_tag(haystack: &str, name: &str) -> Option<usize> {
    let mut search = haystack;
    let mut offset = 0;
    while let Some(pos) = search.find("</") {
        let tag_start = pos + 2;
        let candidate = &search[tag_start..];
        if candidate.starts_with(name) && candidate[name.len()..].starts_with('>') {
            return Some(offset + pos);
        }
        let advance = tag_start;
        offset += advance;
        search = &search[advance..];
    }
    None
}

/// How Claude Code opens the user turn it writes when a skill already loaded
/// in the session is invoked again. The harness wrote it, not the person.
const REINVOKED: &str = "(Re-invocation of /";

/// The name of a skill invoked with `[$name](path)` at the start of `text`.
fn skill_link_name(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("[$")?;
    let (name, after_bracket) = rest.split_once(']')?;
    if name.is_empty() || name.contains('\n') {
        return None;
    }
    let after_paren = after_bracket.strip_prefix('(')?;
    let (path, _) = after_paren.split_once(')')?;
    if path.is_empty() || path.contains('\n') {
        return None;
    }
    Some(name)
}

/// The name of a slash command, `/name`, at the start of `text`. A path such as
/// `/home/me/x` is not a command.
fn slash_command_name(text: &str) -> Option<&str> {
    let first = text.split_whitespace().next()?;
    let name = first.strip_prefix('/')?;
    if !name.is_empty() && !name.contains('/') {
        Some(name)
    } else {
        None
    }
}

/// The skill or command a raw user turn invokes: the `<command-name>` Claude
/// Code writes for a slash command, or a leading `/name` or `[$name](path)`.
fn invoked_name(raw: &str) -> Option<String> {
    let text = raw.trim_start();
    if let Some(rest) = text.strip_prefix(REINVOKED) {
        return rest
            .split([' ', '\u{2014}'])
            .next()
            .map(str::to_owned)
            .filter(|n| !n.is_empty());
    }
    if let Some((_, after_tag)) = text.split_once("<command-name>") {
        if let Some((name_raw, _)) = after_tag.split_once("</command-name>") {
            let name = name_raw.trim().trim_start_matches('/');
            if !name.is_empty() {
                return Some(name.to_owned());
            }
        }
    }
    if let Some(name) = skill_link_name(text) {
        return Some(name.to_owned());
    }
    slash_command_name(text).map(str::to_owned)
}

/// Whether `text` starts by invoking a skill, `[$name](path) ...`, or a slash
/// command, `/name ...`. A path such as `/home/me/x` is not a command.
fn is_invocation(text: &str) -> bool {
    let text = text.trim_start();
    skill_link_name(text).is_some() || slash_command_name(text).is_some()
}

/// Whether `text` asks something: it ends with a question mark, or opens the
/// way a question does.
fn is_question(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    lower.ends_with('?') || QUESTION_OPENERS.iter().any(|q| lower.starts_with(q))
}

/// Negations that tell the agent what not to do.
const NEGATIONS: &[&str] = &["don't", "dont", "didn't", "didnt", "shouldn't", "shouldnt"];

/// Whether `text` insists on something even when it is phrased as a
/// question: it says don't, do not, didn't, or shouldn't, or always or
/// never ("never mind" is not one), or doubts the agent with "are you
/// sure". "why do you always skip the tests?"
/// corrects; "can you check the logs instead" asks.
fn insists(text: &str) -> bool {
    let lower = text.to_lowercase().replace("never mind", "");
    lower.contains("do not ")
        || lower.contains("are you sure")
        || tokens(&lower)
            .iter()
            .any(|t| NEGATIONS.contains(t) || matches!(*t, "always" | "never"))
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
    if opener || use_not {
        return true;
    }
    let padded = format!(" {lower} ");
    PHRASES.iter().any(|p| padded.contains(p)) || says_preference(&lower)
}

/// Whether `text` says the agent's output is too long, too short, too much, or
/// too many, or says what the person only wants, prefers, or would rather.
fn says_preference(text: &str) -> bool {
    let padded = format!(" {} ", text.to_lowercase());
    EXCESS.iter().chain(PREFERENCES).any(|p| padded.contains(p))
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
/// says the output is too long or too many, says what the person prefers or
/// only wants, or says use X not Y. A redirect
/// like "give me the short one instead"
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
        || says_preference(&lower)
        || lower.contains("do not ")
        || use_x_not_y(&words)
        || words
            .iter()
            .any(|w| RULE_WORDS.contains(w) || TOOLS.contains(w))
}

/// Phrases that make a turn read as a standing rule. "make sure" counts for
/// less, and for less still when the same turn holds a task verb (see
/// [`rule_score`]).
const STANDING: &[&str] = &["from now on", "going forward", "always", "never"];
/// Phrases that ask the agent for something now, which a standing rule does
/// not: "can you make sure the prompt is right" is a request.
const ASKS: &[&str] = &[
    " can you ",
    " could you ",
    " can u ",
    " would you ",
    " do you ",
    " do i ",
    " how do ",
    " whats up ",
    " what's up ",
    " why is ",
];
/// More words than this and a turn is usually a task with a rule inside it.
const RULE_WORDS_MAX: usize = 20;
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

/// Verbs that open a one-off task: [`TASK_VERBS`] and the chores a person
/// hands an agent ("delete the runtimes", "install the deps").
const TASK_OPENERS: &[&str] = &[
    "delete",
    "remove",
    "install",
    "uninstall",
    "move",
    "rename",
    "update",
    "add",
    "clean",
];

/// How much `text` reads like a standing rule rather than a one-off task
/// order. Fixed and explainable: +3 for a standing phrase ("from now on",
/// "always", "never", "going forward"; "never mind" is not one), or +2 for
/// "make sure" alone, which earns only +1 when the turn also holds a task
/// verb; -2 when it asks the agent something now (a question mark, "can you",
/// "do you", "how do", ...) without insisting ("why do you always ...?" is
/// not an ask); -1 when it runs past [`RULE_WORDS_MAX`] words;
/// +1 when it names no specific
/// artifact; -3 for each PR or issue number (`#478`, `issue 12`), each
/// branch-shaped token (a slash and no file extension), each URL, and each
/// span the secret scan masked; and -2 when it opens with a task verb
/// ("push", "merge", "fix", ...) and names an artifact; and -5 when it names
/// none but opens with a task verb and adds a second instruction with "and",
/// a task with a rule tacked on ("delete the runtimes and make it build on
/// my ssd from now on").
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
    let standing = lower.replace("never mind", "");
    if STANDING.iter().any(|p| standing.contains(p)) {
        score += 3;
    } else if lower.contains("make sure") {
        score += if task_verb { 1 } else { 2 };
    }
    let padded = format!(" {} ", tokens.join(" "));
    if (lower.contains('?') || ASKS.iter().any(|p| padded.contains(p))) && !insists(&lower) {
        score -= 2;
    }
    if tokens.len() > RULE_WORDS_MAX {
        score -= 1;
    }
    if artifacts == 0 {
        score += 1;
    }
    score -= 3 * i32::try_from(numbers + branches).unwrap_or(i32::MAX / 3);
    let opens_task = tokens.first().is_some_and(|t| TASK_VERBS.contains(t));
    if artifacts > 0 && opens_task {
        score -= 2;
    }
    let opens_chore = tokens.first().is_some_and(|t| TASK_OPENERS.contains(t));
    if artifacts == 0 && (opens_task || opens_chore) && tokens.contains(&"and") {
        score -= 5;
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
    let word = word.trim_end_matches('\'');
    for (suffix, min) in [("ing", 6), ("ed", 5), ("s", 4)] {
        if word.len() >= min && word.ends_with(suffix) && !word.ends_with("ss") {
            return word[..word.len() - suffix.len()]
                .trim_end_matches('\'')
                .to_owned();
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
pub fn lessons_within(candidates: Vec<Candidate>, limits: &Limits) -> Vec<Lesson> {
    let tagged = candidates.into_iter().map(|c| (0, c)).collect();
    let mut found: Vec<Lesson> = groups(tagged, limits)
        .into_iter()
        .filter_map(|group| lesson(group.into_iter().map(|(_, c)| c).collect()))
        .collect();
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

/// `candidates`, each with a tag the caller keeps track of it by, grouped as
/// [`lessons_within`] groups them: oldest first, a resume or fork copy and a
/// candidate with no content words left out, each joining the first group
/// with one of its first [`MATCH_MEMBERS`] members whose content words
/// overlap its own by at least `limits.min_overlap_per_mille`.
fn groups<T>(mut candidates: Vec<(T, Candidate)>, limits: &Limits) -> Vec<Vec<(T, Candidate)>> {
    candidates.sort_by(|(_, a), (_, b)| {
        (a.at_ms, &a.session, &a.text).cmp(&(b.at_ms, &b.session, &b.text))
    });
    // A resume or fork copies the conversation into a new session, user turns
    // and timestamps included. Keep one copy of each, so copies of one
    // conversation never count as two sessions. The earliest session id wins.
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|(_, c)| c.at_ms.is_none() || seen.insert((c.at_ms, c.text.to_lowercase())));
    let mut words_of: Vec<Vec<BTreeSet<String>>> = Vec::new();
    let mut groups: Vec<Vec<(T, Candidate)>> = Vec::new();
    for (tag, candidate) in candidates {
        let words = content_words(&candidate.text);
        if words.is_empty() {
            continue;
        }
        let joins = words_of.iter().position(|group| {
            group
                .iter()
                .take(MATCH_MEMBERS)
                .any(|other| overlap_per_mille(other, &words) >= limits.min_overlap_per_mille)
        });
        match joins {
            Some(at) => {
                words_of[at].push(words);
                groups[at].push((tag, candidate));
            }
            None => {
                words_of.push(vec![words]);
                groups.push(vec![(tag, candidate)]);
            }
        }
    }
    groups
}

/// One group, oldest first, as a lesson, or `None` when it holds no correction
/// or states no rule.
fn lesson(group: Vec<Candidate>) -> Option<Lesson> {
    let oldest = group.first()?.clone();
    let mut receipts: Vec<Candidate> = Vec::new();
    for candidate in group.into_iter().rev() {
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

fn clause_contains_word(clause: &str, word: &str) -> bool {
    let mut search = clause;
    while let Some(pos) = search.find(word) {
        let before_ok = pos > 0 && search.as_bytes()[pos - 1] == b' ';
        let after_pos = pos + word.len();
        let after_ok = after_pos < search.len() && search.as_bytes()[after_pos] == b' ';
        if before_ok && after_ok {
            return true;
        }
        let advance = pos + word.len().max(1);
        search = &search[advance..];
    }
    false
}

/// Whether `text` names `family` by one of its words.
fn names_family(text: &str, words: &[&str]) -> bool {
    clauses(text)
        .iter()
        .any(|clause| words.iter().any(|w| clause_contains_word(clause, w)))
}

/// The words of the first command family `text` names.
fn family_of(text: &str) -> Option<&'static [&'static str]> {
    let clauses = clauses(text);
    clauses.iter().find_map(|clause| {
        let (_, words) = FAMILIES
            .iter()
            .find(|(_, words)| words.iter().any(|w| clause_contains_word(clause, w)))?;
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
pub fn find_conflicts(turns: &Turns, lessons: &mut [Lesson]) {
    let turns = &turns.0;
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
        later.sort_by(|a, b| (b.at_ms, &a.session).cmp(&(a.at_ms, &b.session)));
        // A resume or fork copies a turn into a new session with its
        // timestamp, as in `lessons_within`: keep the earliest session's copy.
        let mut seen = std::collections::HashSet::new();
        later.retain(|t| seen.insert((t.at_ms, one_line(&t.text).to_lowercase())));
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
                following: None,
                signal: Signal::Correction,
            })
            .collect();
    }
}

/// Acknowledgements and connectors that open a turn, cut by [`suggest_rule`].
const ACKS: &[&str] = &[
    "yes", "yea", "yeah", "ok", "okay", "sure", "do that", "and", "also", "so", "then",
];
/// Words that open a correction and are cut only before a comma, since "no"
/// alone is often the rule itself ("no mocks, hit the real database").
const CORRECTION_OPENERS: &[&str] = &[
    "i told you",
    "i said",
    "actually",
    "nope",
    "nah",
    "wait",
    "no",
];
/// Request frames that open a rule, longest first.
const FRAMES: &[&str] = &[
    "make sure that you",
    "make sure that",
    "make sure you",
    "make sure",
    "can you",
    "could you",
    "please",
];
/// Filler that ends a rule.
const TRAILING: &[&str] = &["please", "from now on", "going forward"];
/// Openings of a one-off task order, which no rule is suggested for.
const TASK_FRAMES: &[&str] = &["go ahead"];

/// A lesson's rule in the person's own words, cleaned up by fixed rules:
/// leading acknowledgements and connectors ("yea sure, and also"), a
/// correction opener before a comma ("no,", "I told you,"), and a
/// request frame ("make sure that you", "can you", "please") are cut, then
/// trailing filler ("please", "from now on", "going forward") and
/// punctuation, then "like" and "just like" used as filler between two words
/// (not "look like the old one" or "I like tabs"). "dont" and
/// "cant" get their apostrophes and the first letter is capitalized. Nothing
/// else is reworded. A turn that opens with a task order ("go ahead"), or
/// that leaves fewer than three words, comes back as it was, trimmed.
#[must_use]
pub fn suggest_rule(text: &str) -> String {
    let original = text.trim();
    let mut rest = original;
    loop {
        if let Some(after) = strip_opening(rest, ACKS) {
            rest = after;
        } else if let Some(after) = strip_before_comma(rest, CORRECTION_OPENERS) {
            rest = after;
        } else {
            break;
        }
    }
    while let Some(after) = strip_opening(rest, FRAMES) {
        rest = after;
    }
    if strip_opening(rest, TASK_FRAMES).is_some() {
        return original.to_owned();
    }
    loop {
        let trimmed = rest.trim_end_matches(|c: char| c.is_whitespace() || ",.;:!?".contains(c));
        let cut = TRAILING.iter().find_map(|filler| {
            let at = trimmed.len().checked_sub(filler.len())?;
            let (head, tail) = (trimmed.get(..at)?, trimmed.get(at..)?);
            (tail.eq_ignore_ascii_case(filler) && (head.is_empty() || head.ends_with([' ', ','])))
                .then_some(head)
        });
        rest = cut.unwrap_or(trimmed);
        if cut.is_none() {
            break;
        }
    }
    let words: Vec<&str> = rest.split_whitespace().collect();
    let is = |i: usize, word: &str| words.get(i).is_some_and(|w| w.eq_ignore_ascii_case(word));
    let mut kept = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let filler = |at: usize| !like_compares(words[i - 1], words[at + 1]);
        if i > 0 && is(i, "just") && is(i + 1, "like") && i + 2 < words.len() && filler(i + 1) {
            i += 2;
        } else if i > 0 && is(i, "like") && i + 1 < words.len() && filler(i) {
            i += 1;
        } else {
            kept.push(contract(words[i]));
            i += 1;
        }
    }
    if kept.len() < 3 {
        return original.to_owned();
    }
    let joined = kept.join(" ");
    let mut chars = joined.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Whether "like" between `before` and `after` compares or is a verb, as in
/// "look like the old one" or "I like tabs", rather than filler, as in
/// "can you like run the tests".
fn like_compares(before: &str, after: &str) -> bool {
    const BEFORE: &[&str] = &[
        "look", "looks", "looked", "looking", "feel", "feels", "sound", "sounds", "seem", "seems",
        "is", "are", "was", "were", "be", "been", "i", "you", "we", "they", "would", "dont",
        "don't",
    ];
    const AFTER: &[&str] = &[
        "the", "a", "an", "this", "that", "these", "those", "my", "your", "our", "their", "its",
        "his", "her",
    ];
    let lower = |w: &str| {
        w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
            .to_lowercase()
    };
    BEFORE.contains(&lower(before).as_str()) || AFTER.contains(&lower(after).as_str())
}

/// `text` after the first of `openings` it starts with as whole words, and
/// any commas after it, or `None`.
fn strip_opening<'a>(text: &'a str, openings: &[&str]) -> Option<&'a str> {
    openings.iter().find_map(|opening| {
        let head = text.get(..opening.len())?;
        let after = text.get(opening.len()..)?;
        (head.eq_ignore_ascii_case(opening)
            && after
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || c == ','))
        .then(|| after.trim_start_matches(|c: char| c.is_whitespace() || c == ','))
    })
}

/// `text` after the first of `openings` it starts with when a comma follows
/// it, or `None`.
fn strip_before_comma<'a>(text: &'a str, openings: &[&str]) -> Option<&'a str> {
    openings.iter().find_map(|opening| {
        let head = text.get(..opening.len())?;
        let after = text.get(opening.len()..)?.strip_prefix(',')?;
        head.eq_ignore_ascii_case(opening)
            .then(|| after.trim_start())
    })
}

/// `word` with the apostrophe put back when it is "dont" or "cant".
fn contract(word: &str) -> String {
    match (word.get(..3), word.get(3..4), word.get(4..)) {
        (Some(stem), Some(t), Some(tail))
            if ["don", "can"].iter().any(|s| stem.eq_ignore_ascii_case(s))
                && t.eq_ignore_ascii_case("t")
                && !tail.starts_with(char::is_alphanumeric) =>
        {
            format!("{stem}'{t}{tail}")
        }
        _ => word.to_owned(),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_links_and_slash_commands() {
        assert_eq!(
            skill_link_name("[$code-review](plugins/cr/SKILL.md) please check"),
            Some("code-review")
        );
        assert_eq!(
            skill_link_name("[$日本語](plugins/ja/SKILL.md)"),
            Some("日本語")
        );
        assert_eq!(skill_link_name("[$50 budget] no, use pnpm"), None);
        assert_eq!(skill_link_name("[$50 budget"), None);
        assert_eq!(skill_link_name("[$]()"), None);
        assert_eq!(skill_link_name("[$foo]()"), None);
        assert_eq!(skill_link_name("[$foo\nbar](path)"), None);
        assert_eq!(skill_link_name("[$foo](path\n)"), None);

        assert_eq!(slash_command_name("/review please check"), Some("review"));
        assert_eq!(slash_command_name("/日本語"), Some("日本語"));
        assert_eq!(slash_command_name("/home/me/foo"), None);
        assert_eq!(slash_command_name("/"), None);
        assert_eq!(slash_command_name("review"), None);
        assert_eq!(slash_command_name(""), None);

        assert_eq!(
            invoked_name("<command-name>/review</command-name>"),
            Some("review".to_owned())
        );
        assert_eq!(
            invoked_name("<command-name>review</command-name>"),
            Some("review".to_owned())
        );
        assert_eq!(
            invoked_name("<command-name>/日本語</command-name>"),
            Some("日本語".to_owned())
        );
        assert_eq!(
            invoked_name("do not use <command-name> tags in responses"),
            None
        );
        assert_eq!(
            invoked_name("(Re-invocation of /review — arg)"),
            Some("review".to_owned())
        );
        assert_eq!(
            invoked_name("(Re-invocation of /日本語 arg)"),
            Some("日本語".to_owned())
        );
        assert_eq!(
            invoked_name("[$skill](path) prompt"),
            Some("skill".to_owned())
        );
        assert_eq!(invoked_name("[$50 budget] prompt"), None);

        assert!(is_invocation("[$skill](path) prompt"));
        assert!(is_invocation("[$日本語](path) prompt"));
        assert!(is_invocation("/cmd prompt"));
        assert!(is_invocation("/日本語 prompt"));
        assert!(!is_invocation("[$50 budget] prompt"));
        assert!(!is_invocation("/home/me/path prompt"));
        assert!(!is_invocation("hello world"));
    }

    #[test]
    fn stems_possessive_apostrophes_and_plurals_to_base() {
        assert_eq!(stem("user"), "user");
        assert_eq!(stem("users"), "user");
        assert_eq!(stem("user's"), "user");
        assert_eq!(stem("users'"), "user");
    }
}
