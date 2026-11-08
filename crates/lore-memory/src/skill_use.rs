//! Skill usage: how often each skill or slash command ran, and the corrections
//! the person made right after it, so a skill that keeps needing a second
//! prompt, or one that never runs, can be found.
//!
//! A use is a Claude Code `Skill` tool call (only its `skill` name is read from
//! the call's input) or a user turn that invokes a name, `/name` or
//! `[$name](path)`. A correction is one [`mine::corrections_at`] finds, held
//! against the most recent use in its session when it comes within
//! [`REPROMPT_TURNS`] user turns of it. It writes nothing.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::Connection;

use crate::mine::{self, Candidate, Limits, Turns};

/// A correction counts against a skill when it is among this many user turns
/// after the skill ran. Later than that, the conversation has moved on.
pub const REPROMPT_TURNS: usize = 2;

const SKILL_CALLS: &str = "SELECT m.session_id, m.seq, m.ts, json_extract(tc.input_json, '$.skill')
    FROM tool_call_full tc
    JOIN message_part p ON p.id = tc.call_part_id
    JOIN message m ON m.id = p.message_id
    WHERE tc.name = 'Skill' AND m.is_sidechain = 0
        AND json_extract(tc.input_json, '$.skill') IS NOT NULL
    ORDER BY m.session_id, m.seq";

/// One skill or command name and what came of running it.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillUsage {
    /// As invoked: `code-review`, `plugin:skill`, or a command's name.
    pub skill: String,
    pub uses: usize,
    /// Sessions it ran in.
    pub sessions: usize,
    /// Epoch milliseconds of the newest use.
    pub last_ms: Option<i64>,
    /// Corrections made right after it ran, newest first.
    pub reprompts: Vec<Candidate>,
}

/// The distinct times one name ran, the sessions, and the newest time.
#[derive(Default)]
struct Seen<'a> {
    times: BTreeSet<i64>,
    uses: usize,
    sessions: BTreeSet<&'a str>,
    last_ms: Option<i64>,
}

struct Use {
    session: String,
    seq: i64,
    at_ms: Option<i64>,
    name: String,
}

/// Every name used in the archive, most reprompted first, then most used.
pub fn skill_usage(
    conn: &Connection,
    turns: &Turns,
    limits: &Limits,
) -> rusqlite::Result<Vec<SkillUsage>> {
    let mut uses: Vec<Use> = conn
        .prepare(SKILL_CALLS)?
        .query_map([], |r| {
            Ok(Use {
                session: r.get(0)?,
                seq: r.get(1)?,
                at_ms: r.get(2)?,
                name: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    uses.extend(turns.1.iter().map(|i| Use {
        session: i.session.clone(),
        seq: i.seq,
        at_ms: i.at_ms,
        name: i.name.clone(),
    }));
    uses.sort_by(|a, b| (&a.session, a.seq).cmp(&(&b.session, b.seq)));

    // Walk turns and uses together, in session then turn order, remembering
    // the last use and how many user turns have passed since it.
    let corrections: BTreeMap<usize, Candidate> =
        mine::corrections_at(turns, limits).into_iter().collect();
    let mut held: Vec<(usize, Candidate)> = Vec::new();
    let mut next = 0;
    let mut last: Option<(usize, usize)> = None;
    for (i, turn) in turns.0.iter().enumerate() {
        if last.is_some_and(|(u, _)| uses[u].session != turn.session) {
            last = None;
        }
        while next < uses.len() && (&uses[next].session, uses[next].seq) < (&turn.session, turn.seq)
        {
            if uses[next].session == turn.session {
                last = Some((next, 0));
            }
            next += 1;
        }
        if !turn.user {
            continue;
        }
        if let Some((u, seen)) = last.as_mut() {
            *seen += 1;
            if *seen <= REPROMPT_TURNS {
                if let Some(c) = corrections.get(&i) {
                    held.push((*u, c.clone()));
                }
            }
        }
    }

    // A resumed or forked Claude Code session repeats the turns it copied,
    // timestamps included, so a use or correction seen twice is one.
    let mut by_name: BTreeMap<&str, Seen<'_>> = BTreeMap::new();
    for u in &uses {
        let seen = by_name.entry(&u.name).or_default();
        if match u.at_ms {
            Some(ts) => seen.times.insert(ts),
            None => true,
        } {
            seen.uses += 1;
            seen.sessions.insert(&u.session);
        }
        seen.last_ms = seen.last_ms.max(u.at_ms);
    }
    let mut reprompts: BTreeMap<&str, Vec<Candidate>> = BTreeMap::new();
    for (u, c) in held {
        let list = reprompts.entry(&uses[u].name).or_default();
        if !list.iter().any(|x| x.text == c.text && x.at_ms == c.at_ms) {
            list.push(c);
        }
    }
    let mut out: Vec<SkillUsage> = by_name
        .into_iter()
        .map(|(name, seen)| {
            let mut reprompts = reprompts.remove(name).unwrap_or_default();
            reprompts.sort_by_key(|c| std::cmp::Reverse(c.at_ms));
            SkillUsage {
                skill: name.to_owned(),
                uses: seen.uses,
                sessions: seen.sessions.len(),
                last_ms: seen.last_ms,
                reprompts,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.reprompts
            .len()
            .cmp(&a.reprompts.len())
            .then(b.uses.cmp(&a.uses))
            .then(a.skill.cmp(&b.skill))
    });
    Ok(out)
}
