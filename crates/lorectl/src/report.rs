//! `lorectl report`: what corrections a person gave agents over a window,
//! which became lessons, whether the lessons reached agents, and whether a
//! mistake was corrected again after a lesson about it was accepted.
//!
//! Read only on the archive and on the vault. The vault's index is not used,
//! because bringing it up to date can write; the notes are read from their
//! files into an index held in memory instead.
//!
//! Every section names where its counts come from:
//! - Activity: user turns with text in the archive's messages.
//! - Corrections: the same miner as `lorectl mem mine`, run over only the
//!   sessions with a message in the window (each session whole), and the
//!   vault's `.lore/considered.jsonl` ledger of corrections a finished Find
//!   lessons run read.
//! - Lessons: proposals in `_inbox/`, decisions in `.lore/decisions.jsonl`,
//!   and the note files.
//! - Delivery: the `.lore/shown/` records the hooks write, one per session.
//!   They hold a hash of the session id, and the hooks prune them after
//!   seven days, so a longer window cannot see older deliveries.
//! - Repeats: rule-flagged corrections that share the lesson of an active
//!   note accepted strictly before them. Notes are found by BM25 with the
//!   rule `lorectl hook prompt` uses (two shared index words), and a match
//!   then also needs, by [`related_words`], two words shared with the
//!   note's lesson (its title and the statement above `Receipts:`), or two
//!   shared with one of its receipts of which one is a lesson word. A
//!   receipt's other words, such as "make sure", were said around the
//!   lesson and match turns about anything. This is an estimate from word
//!   overlap.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};

use lexopt::prelude::*;
use lore_core::enrich::{resolve_repository, RepoResolution};
use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::storage::{self, StorageError};
use lore_memory::considered::{self, CorrectionId};
use lore_memory::inbox;
use lore_memory::index::{related_words, split_timeline, stem, without_receipts, Index};
use lore_memory::mine::{self, Candidate};
use lore_memory::note::{Note, Status};
use lore_memory::repo::RepoContext;
use lore_memory::review::{self, Decision};
use lore_memory::shown;
use lore_memory::vault::{self, Vault};
use serde_json::{json, Value};

use crate::cli::Invocation;
use crate::exit::{self, CliError};
use crate::receipt::date;

/// The window when `--since` is not given.
const DEFAULT_SINCE: &str = "7d";
/// Lines in each short list.
const LIST: usize = 5;
/// Characters of a correction's excerpt under Repeats.
const EXCERPT_CHARS: usize = 80;
/// Notes checked per correction for an earlier accepted one.
const REPEAT_CANDIDATES: usize = 5;
const DAY_MS: i64 = 86_400_000;

/// `lorectl help report`, and `lorectl report --help`.
pub const USAGE: &str = "\
Report (lorectl report [--since <7d|30d|YYYY-MM-DD>] [--cwd DIR | --all-repos] [--json]):
    What corrections you gave agents in the window, which became lessons,
    whether the lessons were shown to agents, and which mistakes were
    corrected again after a note about them was accepted.
    Activity          Sessions and user turns by agent, top repositories
    Corrections       What `lorectl mem mine` finds in the window, and how
                      many a finished Find lessons run already read
    Lessons           Proposals filed, accepted, and rejected in the window,
                      pending now, and active notes
    Delivery          Sessions shown a note by the hooks, and the notes
                      shown most; the hooks keep 7 days of these records
    Repeats           Corrections sharing two content words with an active
                      note accepted before them; an estimate from word
                      overlap
    --since <WINDOW>  Days (7d), hours (12h), minutes (90m), or a start date
                      (UTC); default 7d
    --cwd <DIR>       Only the repository at DIR; lessons and delivery stay
                      vault-wide
    --all-repos       Every repository; the default
    --vault <DIR>     Vault directory; otherwise LORE_VAULT_DIR or ~/Lore
    --json            The same data as one JSON object
    Reads the archive and the vault and writes neither. Exits 0 with a short
    explanation when there is no archive or no vault yet.
";

#[derive(Debug, Default)]
struct Args {
    since: Option<String>,
    cwd: Option<PathBuf>,
    all_repos: bool,
    vault: Option<PathBuf>,
    archive: Option<PathBuf>,
    json: bool,
    help: bool,
}

fn usage(error: impl ToString) -> CliError {
    CliError::Usage(error.to_string())
}

fn parse(invocation: &Invocation) -> Result<Args, CliError> {
    let mut args = Args {
        since: invocation.since.clone(),
        vault: invocation.vault.clone(),
        archive: invocation.archive.clone(),
        json: invocation.json,
        ..Args::default()
    };
    let mut parser = lexopt::Parser::from_args(&invocation.rest);
    while let Some(arg) = parser.next().map_err(usage)? {
        match arg {
            Short('h') | Long("help") => args.help = true,
            Long("since") => {
                args.since = Some(parser.value().map_err(usage)?.to_string_lossy().into())
            }
            Long("cwd") => args.cwd = Some(parser.value().map_err(usage)?.into()),
            Long("all-repos") => args.all_repos = true,
            Long("vault") => args.vault = Some(parser.value().map_err(usage)?.into()),
            Long("archive") => args.archive = Some(parser.value().map_err(usage)?.into()),
            Long("json") => args.json = true,
            other => return Err(usage(other.unexpected())),
        }
    }
    if args.cwd.is_some() && args.all_repos {
        return Err(usage("--cwd and --all-repos cannot be used together"));
    }
    Ok(args)
}

/// The start of the window in epoch milliseconds, from `--since`.
fn window_start(given: &str, now_ms: i64) -> Result<i64, CliError> {
    if let Some(start) = day_start_ms(given) {
        return Ok(start);
    }
    let bad = || {
        usage(format!(
            "--since `{given}` must be like 7d, 30d, 12h, 90m, or 2026-11-01"
        ))
    };
    let (_, at) = crate::scan::window(given).map_err(|_| bad())?;
    let back = std::time::SystemTime::now()
        .duration_since(at)
        .map_err(|_| bad())?;
    Ok(now_ms.saturating_sub(i64::try_from(back.as_millis()).unwrap_or(i64::MAX)))
}

/// Midnight UTC of a `YYYY-MM-DD` date, in epoch milliseconds.
fn day_start_ms(text: &str) -> Option<i64> {
    let mut parts = text.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let month = time::Month::try_from(m.parse::<u8>().ok()?).ok()?;
    let day = time::Date::from_calendar_date(y.parse().ok()?, month, d.parse().ok()?).ok()?;
    Some(day.midnight().assume_utc().unix_timestamp() * 1000)
}

/// Run `lorectl report`.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let args = parse(invocation)?;
    if args.help {
        print!("{USAGE}");
        return Ok(exit::OK);
    }
    let now = lore_core::now_ms();
    let window = args.since.clone().unwrap_or_else(|| DEFAULT_SINCE.into());
    let start = window_start(&window, now)?;
    let archive_dir = lore_core::paths::archive_dir(args.archive.as_deref())?;
    let db = archive_dir.join(ARCHIVE_DB_FILENAME);
    let mut out = std::io::stdout().lock();
    if !db.exists() {
        let explanation = format!(
            "No archive at {} yet, so there is nothing to report. \
             Run `lorectl init` or `lorectl scan` first.",
            archive_dir.display()
        );
        if args.json {
            let _ = writeln!(
                out,
                "{}",
                json!({ "archive": null, "explanation": explanation })
            );
        } else {
            let _ = writeln!(out, "{explanation}");
        }
        return Ok(exit::OK);
    }
    let conn = storage::open_read_only(&db)?;
    let scope = match &args.cwd {
        None => Scope::All,
        Some(dir) => match resolve_repository(&conn, dir)? {
            RepoResolution::NotARepository => return Err(CliError::NotARepository),
            RepoResolution::InArchive {
                repository_id,
                display_name,
                ..
            } => Scope::Repo {
                id: Some(repository_id),
                name: display_name,
            },
            RepoResolution::NotInArchive { display_name, .. } => Scope::Repo {
                id: None,
                name: display_name,
            },
        },
    };

    let activity = activity(&conn, start, &scope)?;
    // Only the sessions with a message in the window are mined, each whole,
    // so the cost follows the window and not the size of the archive.
    let turns = mine::read_turns_since(&conn, start).map_err(StorageError::from)?;
    let limits = mine::Limits::default();
    let in_scope = |c: &Candidate| {
        c.at_ms.is_some_and(|at| at >= start)
            && match &scope {
                Scope::All => true,
                Scope::Repo { name, .. } => c.repo.as_deref() == Some(name.as_str()),
            }
    };
    let flagged: Vec<Candidate> = mine::corrections_in(&turns, &limits)
        .into_iter()
        .filter(|c| in_scope(c))
        .collect();
    // A flagged correction whose lesson the miner drops is offered as a reply
    // too; it is counted once, as flagged.
    let counted: std::collections::HashSet<(&str, Option<i64>, &str)> = (flagged.iter())
        .map(|c| (c.session.as_str(), c.at_ms, c.text.as_str()))
        .collect();
    let replies: Vec<Candidate> = mine::replies_in(&turns, &limits, start)
        .into_iter()
        .filter(|c| in_scope(c))
        .filter(|c| !counted.contains(&(c.session.as_str(), c.at_ms, c.text.as_str())))
        .collect();
    drop(turns);

    let vault_dir = vault::resolve_dir(args.vault.as_deref()).ok();
    let vault = vault_dir.as_deref().and_then(|dir| Vault::open(dir).ok());
    let report = Report {
        window,
        start,
        now,
        scope,
        activity,
        corrections: corrections(&flagged, &replies, vault.as_ref())?,
        vault: match &vault {
            Some(vault) => Some(vault_sections(vault, start, &flagged)?),
            None => None,
        },
        vault_dir,
    };
    let text = if args.json {
        format!("{}\n", report.json())
    } else {
        report.text()
    };
    let _ = out.write_all(text.as_bytes());
    Ok(exit::OK)
}

enum Scope {
    All,
    Repo { id: Option<String>, name: String },
}

#[derive(Default)]
struct Activity {
    sessions: i64,
    turns: i64,
    /// `(agent, sessions, turns)`, most sessions first.
    by_agent: Vec<(String, i64, i64)>,
    /// `(repository, sessions)`, most first, at most [`LIST`].
    repos: Vec<(String, i64)>,
}

struct Corrections {
    flagged: usize,
    replies: usize,
    /// Of `flagged` and `replies`, how many the considered ledger names.
    read: usize,
    /// `flagged` by signal: correction, repeat, rule.
    by_signal: BTreeMap<&'static str, usize>,
}

struct Lessons {
    filed: usize,
    accepted: usize,
    rejected: usize,
    pending: usize,
    active: usize,
}

struct Delivery {
    sessions: usize,
    notes: usize,
    /// `(title, sessions)`, most first, at most [`LIST`].
    top: Vec<(String, usize)>,
}

struct Repeat {
    title: String,
    note_id: String,
    at_ms: i64,
    excerpt: String,
}

struct VaultSections {
    lessons: Lessons,
    delivery: Delivery,
    repeats: Vec<Repeat>,
    /// Rule-flagged corrections with two content words or more, the ones
    /// Repeats checked.
    checked: usize,
}

struct Report {
    window: String,
    start: i64,
    now: i64,
    scope: Scope,
    activity: Activity,
    corrections: Corrections,
    vault: Option<VaultSections>,
    vault_dir: Option<PathBuf>,
}

/// User turns with text, from `start` on, by agent and by repository. Turns
/// of a session a program started (`claude -p`, `codex exec`) are left out:
/// a program wrote them, not the person.
fn activity(conn: &rusqlite::Connection, start: i64, scope: &Scope) -> Result<Activity, CliError> {
    let (filter, repo) = match scope {
        Scope::All => ("", None),
        // A repository the archive does not know has no activity in it.
        Scope::Repo { id, .. } => (
            " AND sg.repository_id = ?2",
            Some(id.clone().unwrap_or_default()),
        ),
    };
    let from = format!(
        "FROM message m
         JOIN agent_session s ON s.id = m.session_id
         LEFT JOIN session_segment sg ON sg.id = m.segment_id
         LEFT JOIN repository r ON r.id = sg.repository_id
         WHERE m.role = 'user' AND m.is_sidechain = 0 AND m.ts >= ?1{filter}
           AND coalesce(json_extract(s.metadata_json, '$.automated'), 0) = 0
           AND EXISTS (SELECT 1 FROM message_part p WHERE p.message_id = m.id AND p.kind = 'text')"
    );
    let sql = |e: rusqlite::Error| CliError::from(StorageError::from(e));
    let query = |text: &str| -> Result<Vec<(String, i64, i64)>, CliError> {
        let mut statement = conn.prepare(text).map_err(sql)?;
        let map = |r: &rusqlite::Row<'_>| Ok((r.get(0)?, r.get(1)?, r.get(2)?));
        let rows = match &repo {
            None => statement.query_map(rusqlite::params![start], map),
            Some(id) => statement.query_map(rusqlite::params![start, id], map),
        }
        .map_err(sql)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(sql)
    };
    let totals = query(&format!(
        "SELECT '', count(DISTINCT m.session_id), count(*) {from}"
    ))?;
    let by_agent = query(&format!(
        "SELECT s.agent_id, count(DISTINCT m.session_id), count(*) {from}
         GROUP BY s.agent_id ORDER BY 2 DESC, 1"
    ))?;
    let repos = query(&format!(
        "SELECT coalesce(r.display_name, '(no repository)'), count(DISTINCT m.session_id), 0 {from}
         GROUP BY r.id ORDER BY 2 DESC, 1 LIMIT {LIST}"
    ))?;
    let (sessions, turns) = totals.first().map_or((0, 0), |t| (t.1, t.2));
    Ok(Activity {
        sessions,
        turns,
        by_agent,
        repos: repos.into_iter().map(|(name, n, _)| (name, n)).collect(),
    })
}

fn corrections(
    flagged: &[Candidate],
    replies: &[Candidate],
    vault: Option<&Vault>,
) -> Result<Corrections, CliError> {
    let seen = match vault {
        Some(vault) => considered::considered(vault)?,
        None => Default::default(),
    };
    let read = flagged
        .iter()
        .chain(replies)
        .filter(|c| seen.contains(&CorrectionId::of(c)))
        .count();
    let mut by_signal = BTreeMap::new();
    for c in flagged {
        *by_signal.entry(c.signal.as_str()).or_insert(0) += 1;
    }
    Ok(Corrections {
        flagged: flagged.len(),
        replies: replies.len(),
        read,
        by_signal,
    })
}

fn vault_sections(
    vault: &Vault,
    start: i64,
    flagged: &[Candidate],
) -> Result<VaultSections, CliError> {
    let review_error = |e: lore_memory::review::ReviewError| CliError::from(e);
    let notes: Vec<(String, Note)> = vault.notes()?;
    let decisions = review::decisions_at(vault)?;
    let ms = |at: &time::OffsetDateTime| {
        i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
    };

    // Lessons.
    let (proposals, _) = inbox::proposals(vault).map_err(review::ReviewError::from)?;
    let filed = proposals
        .iter()
        .filter(|p| ms(&p.proposed_at) >= start)
        .count();
    let (mut accepted, mut rejected) = (0, 0);
    // When each note was accepted, from its decision; the earliest wins.
    let mut accepted_at: HashMap<String, i64> = HashMap::new();
    for (decision, at) in decisions.values() {
        let at = at.as_ref().map(ms);
        let in_window = at.is_some_and(|at| at >= start);
        match decision {
            Decision::Accepted { id, .. } => {
                accepted += usize::from(in_window);
                if let Some(at) = at {
                    let slot = accepted_at.entry(id.as_str().to_owned()).or_insert(at);
                    *slot = (*slot).min(at);
                }
            }
            Decision::Rejected { .. } => rejected += usize::from(in_window),
        }
    }
    let pending = review::inbox(vault).map_err(review_error)?.proposals.len();
    let active: Vec<&(String, Note)> = notes
        .iter()
        .filter(|(_, n)| n.status == Status::Active)
        .collect();

    // Delivery.
    let title: HashMap<&str, &str> = notes
        .iter()
        .map(|(_, n)| (n.id.as_str(), n.title.as_str()))
        .collect();
    let since =
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(start.max(0).unsigned_abs());
    let records: Vec<Vec<String>> = shown::records(vault)
        .into_iter()
        .filter(|(at, _)| *at >= since)
        .map(|(_, ids)| ids)
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for id in records.iter().flatten() {
        *counts.entry(id.as_str()).or_insert(0) += 1;
    }
    let mut top: Vec<(String, usize)> = counts
        .iter()
        .map(|(id, n)| {
            (
                title
                    .get(id)
                    .map_or_else(|| (*id).to_owned(), |t| (*t).to_owned()),
                *n,
            )
        })
        .collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top.truncate(LIST);
    let delivery = Delivery {
        sessions: records.iter().filter(|ids| !ids.is_empty()).count(),
        notes: counts.len(),
        top,
    };

    // Repeats. A note with no accept decision (added by hand, or before
    // decisions were kept) counts as accepted at the end of its `created` day.
    let index = Index::in_memory(active.iter().map(|(rel, n)| (rel.as_str(), n)))
        .map_err(|e| CliError::from(vault::VaultError::from(e)))?;
    let since_accept: HashMap<&str, i64> = active
        .iter()
        .map(|(_, n)| {
            let id = n.id.as_str();
            let at = accepted_at.get(id).copied().unwrap_or_else(|| {
                day_start_ms(&n.created.to_string()).map_or(i64::MAX, |d| d + DAY_MS)
            });
            (id, at)
        })
        .collect();
    let words: HashMap<&str, NoteWords> = active
        .iter()
        .map(|(_, n)| (n.id.as_str(), NoteWords::of(n)))
        .collect();
    let mut scopes: HashMap<String, Option<String>> = HashMap::new();
    let mut repeats = Vec::new();
    let mut checked = 0;
    for c in flagged {
        let Some(at) = c.at_ms else { continue };
        if mine::content_words(&c.text).len() < 2 {
            continue;
        }
        let said = stems(&c.text);
        checked += 1;
        let scope = match &c.repo_path {
            Some(path) => scopes
                .entry(path.clone())
                .or_insert_with(|| {
                    RepoContext::discover(Path::new(path))
                        .key()
                        .map(|key| format!("repo:{}", key.as_str()))
                })
                .clone(),
            None => None,
        };
        let hits = index
            .search_active_related(&c.text, scope.as_deref(), REPEAT_CANDIDATES)
            .map_err(|e| CliError::from(vault::VaultError::from(e)))?;
        let earlier = hits.iter().find(|hit| {
            since_accept.get(hit.id.as_str()).is_some_and(|&a| a < at)
                && words.get(hit.id.as_str()).is_some_and(|w| w.matches(&said))
        });
        if let Some(hit) = earlier {
            let excerpt: String = inbox::redact(&c.text)
                .unwrap_or_else(|_| "(could not redact)".into())
                .chars()
                .take(EXCERPT_CHARS)
                .collect();
            repeats.push(Repeat {
                title: hit.title.clone(),
                note_id: hit.id.clone(),
                at_ms: at,
                excerpt,
            });
        }
    }
    repeats.sort_by_key(|r| std::cmp::Reverse(r.at_ms));
    Ok(VaultSections {
        lessons: Lessons {
            filed,
            accepted,
            rejected,
            pending,
            active: active.len(),
        },
        delivery,
        repeats,
        checked,
    })
}

/// The content words of `text`, by [`related_words`], with plural and verb
/// endings folded so "permissions" is "permission".
fn stems(text: &str) -> BTreeSet<String> {
    related_words(text)
        .iter()
        .filter(|w| !REPEAT_FILLER.contains(&w.as_str()))
        .map(|w| stem(w))
        .collect()
}

/// Words that say how a rule applies, not what it is about: "in each pr make
/// sure to add images" is not a repeat of "Give each change its own PR". And
/// the words every lesson's statement uses ("the agent did this, and the
/// person wanted that"), which tie a correction to whichever note it meets.
const REPEAT_FILLER: &[&str] = &[
    "each", "every", "make", "own", "agent", "agents", "person", "want", "wanted", "wants",
];

/// The words of a note a correction must share to repeat it, by
/// [`stems`]: its lesson (the title and the statement above
/// `Receipts:`), and each receipt (a quoted correction under `Receipts:`).
struct NoteWords {
    lesson: BTreeSet<String>,
    receipts: Vec<BTreeSet<String>>,
}

impl NoteWords {
    fn of(note: &Note) -> Self {
        let statement = split_timeline(&note.body).0;
        let above = without_receipts(statement);
        // related_words keeps at most a dozen words of a text, so a long
        // lesson is read a line at a time.
        let words = |text: &str| -> BTreeSet<String> { text.lines().flat_map(stems).collect() };
        let lesson = words(&format!("{}\n{above}", note.title));
        let receipts = statement
            .get(above.len()..)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("- "))
            .map(|line| {
                // The quote, when the receipt has one: `agent session date: "..."`.
                let quote = match (line.find('"'), line.rfind('"')) {
                    (Some(a), Some(b)) if b > a => &line[a + 1..b],
                    _ => line,
                };
                words(quote)
            })
            .filter(|w| !w.is_empty())
            .collect();
        Self { lesson, receipts }
    }

    /// Whether `said` shares two words with the lesson, or two with one
    /// receipt where at least one is also a lesson word. A quote's other
    /// words were said around the lesson, often about something else ("make
    /// sure"), so on their own they never make a repeat.
    fn matches(&self, said: &BTreeSet<String>) -> bool {
        if self.lesson.intersection(said).nth(1).is_some() {
            return true;
        }
        self.receipts.iter().any(|quote| {
            let shared: Vec<&String> = quote.intersection(said).collect();
            shared.len() >= 2 && shared.iter().any(|w| self.lesson.contains(*w))
        })
    }
}

fn day(ms: i64) -> String {
    date(ms).unwrap_or_else(|| ms.to_string())
}

fn plural(n: impl Into<i64>, one: &str, many: &str) -> String {
    let n = n.into();
    format!("{n} {}", if n == 1 { one } else { many })
}

fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl Report {
    fn scope_name(&self) -> String {
        match &self.scope {
            Scope::All => "all repositories".into(),
            Scope::Repo { name, .. } => format!("repository {name}"),
        }
    }

    fn text(&self) -> String {
        let mut o = format!(
            "Lore report, {} to {} ({}), {}\n",
            day(self.start),
            day(self.now),
            self.window,
            self.scope_name()
        );
        let a = &self.activity;
        o.push_str("\nActivity, from user turns with text in the archive\n");
        o.push_str(&format!(
            "  {}, {}\n",
            plural(a.sessions, "session", "sessions"),
            plural(a.turns, "user turn", "user turns")
        ));
        for (agent, sessions, turns) in a.by_agent.iter().take(LIST) {
            o.push_str(&format!(
                "  - {agent}: {}, {}\n",
                plural(*sessions, "session", "sessions"),
                plural(*turns, "turn", "turns")
            ));
        }
        if !a.repos.is_empty() {
            let repos: Vec<String> = a.repos.iter().map(|(r, n)| format!("{r} {n}")).collect();
            o.push_str(&format!(
                "  top repositories by sessions: {}\n",
                repos.join(", ")
            ));
        }

        let c = &self.corrections;
        o.push_str(
            "\nCorrections, from the `lorectl mem mine` miner over sessions active in the window\n",
        );
        let signals: Vec<String> = c
            .by_signal
            .iter()
            .map(|(s, n)| format!("{n} {s}"))
            .collect();
        o.push_str(&format!(
            "  {} flagged by a rule{}\n",
            c.flagged,
            if signals.is_empty() {
                String::new()
            } else {
                format!(" ({})", signals.join(", "))
            }
        ));
        o.push_str(&format!(
            "  {} reply candidates no rule flagged\n",
            c.replies
        ));
        o.push_str(&format!(
            "  {} of these already read by a finished Find lessons run (considered.jsonl)\n",
            c.read
        ));

        let Some(v) = &self.vault else {
            let at = self
                .vault_dir
                .as_ref()
                .map_or_else(|| "the vault".into(), |d| d.display().to_string());
            o.push_str(&format!(
                "\nNo vault at {at}, so there are no lessons, deliveries, or repeats to report. \
                 Run `lorectl init` or `lorectl mem init` to make one.\n"
            ));
            return o;
        };
        let l = &v.lessons;
        o.push_str("\nLessons, from the vault's inbox, decisions.jsonl, and note files\n");
        o.push_str(&format!(
            "  {} filed, {} accepted, {} rejected in the window (vault-wide)\n",
            l.filed, l.accepted, l.rejected
        ));
        o.push_str(&format!(
            "  {} pending now, {} in total\n",
            plural(count(l.pending), "proposal", "proposals"),
            plural(count(l.active), "active note", "active notes")
        ));

        let d = &v.delivery;
        o.push_str("\nDelivery, from the hooks' .lore/shown records (vault-wide)\n");
        o.push_str(&format!(
            "  {} shown {} by a hook\n",
            plural(count(d.sessions), "session was", "sessions were"),
            plural(count(d.notes), "distinct note", "distinct notes")
        ));
        for (title, n) in &d.top {
            o.push_str(&format!(
                "  - {title} ({})\n",
                plural(count(*n), "session", "sessions")
            ));
        }
        if self.now - self.start > shown::MAX_AGE.as_millis() as i64 {
            o.push_str(
                "  the hooks keep these records 7 days, so older deliveries are not counted\n",
            );
        }

        o.push_str(
            "\nRepeats, an estimate from word overlap between corrections and active notes\n",
        );
        o.push_str(&format!(
            "  {} of {} rule-flagged corrections with two content words or more \
             match a note accepted before them\n",
            v.repeats.len(),
            v.checked
        ));
        for r in v.repeats.iter().take(LIST) {
            o.push_str(&format!(
                "  - {}  {}  \"{}\"\n",
                r.title,
                day(r.at_ms),
                r.excerpt
            ));
        }
        if v.repeats.len() > LIST {
            o.push_str(&format!(
                "  - and {} more (--json lists them)\n",
                v.repeats.len() - LIST
            ));
        }
        o
    }

    fn json(&self) -> Value {
        let a = &self.activity;
        let c = &self.corrections;
        let (repo, repository_id) = match &self.scope {
            Scope::All => (Value::Null, Value::Null),
            Scope::Repo { id, name } => (json!(name), json!(id)),
        };
        let vault = self.vault.as_ref().map(|v| {
            json!({
                "lessons": {
                    "source": "vault _inbox, .lore/decisions.jsonl, note files",
                    "filed": v.lessons.filed, "accepted": v.lessons.accepted,
                    "rejected": v.lessons.rejected, "pending": v.lessons.pending,
                    "active_notes": v.lessons.active,
                },
                "delivery": {
                    "source": "vault .lore/shown, kept 7 days",
                    "sessions": v.delivery.sessions, "distinct_notes": v.delivery.notes,
                    "top": v.delivery.top.iter().map(|(title, n)| json!({
                        "title": title, "sessions": n,
                    })).collect::<Vec<_>>(),
                },
                "repeats": {
                    "source": "estimate: BM25 word overlap with active notes accepted earlier",
                    "checked": v.checked,
                    "count": v.repeats.len(),
                    "items": v.repeats.iter().map(|r| json!({
                        "note_title": r.title, "note_id": r.note_id,
                        "date": date(r.at_ms), "excerpt": r.excerpt,
                    })).collect::<Vec<_>>(),
                },
            })
        });
        json!({
            "window": self.window,
            "start": date(self.start),
            "end": date(self.now),
            "start_ms": self.start,
            "end_ms": self.now,
            "repository": repo,
            "repository_id": repository_id,
            "activity": {
                "source": "archive user turns with text",
                "sessions": a.sessions, "user_turns": a.turns,
                "by_agent": a.by_agent.iter().map(|(agent, s, t)| json!({
                    "agent": agent, "sessions": s, "user_turns": t,
                })).collect::<Vec<_>>(),
                "top_repositories": a.repos.iter().map(|(r, n)| json!({
                    "repository": r, "sessions": n,
                })).collect::<Vec<_>>(),
            },
            "corrections": {
                "source": "lorectl mem mine miner over sessions active in the window; vault .lore/considered.jsonl",
                "flagged": c.flagged,
                "by_signal": c.by_signal,
                "replies": c.replies,
                "already_read": c.read,
            },
            "vault": self.vault_dir.as_ref().map(|d| d.display().to_string()),
            "lessons": vault.as_ref().map(|v| v["lessons"].clone()),
            "delivery": vault.as_ref().map(|v| v["delivery"].clone()),
            "repeats": vault.as_ref().map(|v| v["repeats"].clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_window_starts_at_midnight_utc() {
        assert_eq!(day_start_ms("1970-01-02"), Some(DAY_MS));
        assert_eq!(day_start_ms("2026-13-01"), None);
        assert_eq!(day_start_ms("7d"), None);
    }

    #[test]
    fn a_duration_window_counts_back_from_now() {
        let now = lore_core::now_ms();
        let start = window_start("7d", now).unwrap();
        assert!((now - start - 7 * DAY_MS).abs() < 5_000, "{}", now - start);
        assert!(window_start("soon", now).is_err());
    }
}
