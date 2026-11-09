//! Find lessons on its own: soon after the person corrects their agent, the
//! app runs the same pinned Find lessons run the button starts, at most once a
//! day and only when there are corrections no earlier run has read. Only a run
//! that finished counts toward the day: one that failed, timed out, or could
//! not start is tried again an hour later, up to [`MAX_FAILURES`] times a day.
//!
//! Ingest events and a [`DueTimer`] tick ([`check`]) only look at one setting
//! row. Mining and the run happen on a thread of their own, so neither waits
//! on either. The timer is what starts a run that becomes due after the last
//! session was ingested. One [`LessonsLock`] is held by whichever run is
//! going, manual or automatic, so two never overlap.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use lore_ipc::AutoLessonsDto;
use lore_memory::considered::CorrectionId;
use lore_memory::review;
use lore_memory::vault::Vault;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use super::lessons::{self, Agent, Prepared};
use crate::state::AppState;

/// The setting that holds [`AutoLessons`], as JSON.
pub(crate) const KEY: &str = "lessons.auto";
/// The event the UI hears when an automatic run starts or ends.
pub(crate) const EVENT: &str = "auto_lessons";
/// At most one automatic run finishes in this long.
pub(crate) const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// A check that found nothing new is not repeated for this long, so a stream
/// of watcher events does not mine the archive again and again. A run that
/// did not finish is tried again after this long, too.
pub(crate) const HOUR_MS: i64 = 60 * 60 * 1000;
/// After this many attempts in a row did not finish, no more are made until a
/// day after the first of them, so a lasting failure does not spend usage
/// every hour.
pub(crate) const MAX_FAILURES: u32 = 3;
/// An automatic Claude Code run uses Sonnet, not the person's default model,
/// to keep a run they did not ask for cheap. Codex keeps its own default.
const CLAUDE_MODEL: &str = "sonnet";
/// An outcome is one line in Settings; a longer error is cut.
const OUTCOME_CHARS: usize = 200;
/// How long after launch the [`DueTimer`] first checks.
pub(crate) const FIRST_TICK: Duration = Duration::from_secs(60);
/// How often the [`DueTimer`] checks after that.
pub(crate) const TICK: Duration = Duration::from_secs(15 * 60);

/// The automatic run's setting and what it last did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AutoLessons {
    /// On unless the person turned it off.
    pub enabled: bool,
    /// When an automatic run last finished: its agent exited successfully and
    /// the ledger recorded what it read.
    pub last_finished_ms: Option<i64>,
    /// When an automatic run last started, or last failed to finish or start.
    pub last_attempt_ms: Option<i64>,
    /// Attempts in a row that did not finish. A run counts as one when it
    /// starts, and stops counting when it finishes, so a run cut off by the
    /// app quitting counts too.
    pub consecutive_failures: u32,
    /// When the first of those attempts started.
    pub failing_since_ms: Option<i64>,
    /// When Lore last looked for new corrections on its own.
    pub last_check_ms: Option<i64>,
    /// What that look led to, in a short sentence.
    pub last_outcome: Option<String>,
    /// The agent the person last ran Find lessons with, as `find_lessons` takes it.
    pub agent: Option<String>,
    /// When an automatic run last started an agent, as stored before runs that
    /// did not finish were told apart. [`AutoLessons::load`] moves it into the
    /// fields above; it is never written.
    #[serde(rename = "last_run_ms", skip_serializing)]
    legacy_last_run_ms: Option<i64>,
}

impl Default for AutoLessons {
    fn default() -> Self {
        Self {
            enabled: true,
            last_finished_ms: None,
            last_attempt_ms: None,
            consecutive_failures: 0,
            failing_since_ms: None,
            last_check_ms: None,
            last_outcome: None,
            agent: None,
            legacy_last_run_ms: None,
        }
    }
}

/// Whether `outcome` is what [`work`] stores for a run that finished.
fn finished_outcome(outcome: &str) -> bool {
    outcome.contains(" new correction") && outcome.contains(" and filed ")
}

impl AutoLessons {
    /// The stored setting; the defaults when it is unset or unreadable.
    fn load(conn: &Connection) -> Self {
        lore_core::settings::get(conn, KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<Self>(&raw).ok())
            .unwrap_or_default()
            .migrated()
    }

    /// Move an old `last_run_ms` into the new fields: a run whose stored
    /// outcome says it filed lessons finished; any other was an attempt that
    /// did not.
    fn migrated(mut self) -> Self {
        let Some(t) = self.legacy_last_run_ms.take() else {
            return self;
        };
        if self.last_finished_ms.is_some() || self.last_attempt_ms.is_some() {
            return self;
        }
        if self.last_outcome.as_deref().is_some_and(finished_outcome) {
            self.last_finished_ms = Some(t);
        } else {
            self.last_attempt_ms = Some(t);
            self.consecutive_failures = 1;
            self.failing_since_ms = Some(t);
        }
        self
    }

    /// An automatic run starts at `now_ms`. It counts as a failure until it
    /// finishes; a day after the first failure in a row, the count starts over.
    fn begin_attempt(&mut self, now_ms: i64) {
        let window_over = self
            .failing_since_ms
            .is_none_or(|t| now_ms >= t.saturating_add(DAY_MS));
        if self.consecutive_failures == 0 || window_over {
            self.consecutive_failures = 1;
            self.failing_since_ms = Some(now_ms);
        } else {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        }
        self.last_attempt_ms = Some(now_ms);
    }

    /// The run that started last finished at `now_ms`.
    fn finish_attempt(&mut self, now_ms: i64) {
        self.last_finished_ms = Some(now_ms);
        self.consecutive_failures = 0;
        self.failing_since_ms = None;
    }

    /// The run that started last failed at `now_ms`; the hour counts from here.
    fn fail_attempt(&mut self, now_ms: i64) {
        self.last_attempt_ms = Some(now_ms);
    }

    fn save(&self, conn: &Connection) -> Result<(), String> {
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        lore_core::settings::set(conn, KEY, &json).map_err(|e| e.to_string())
    }

    fn dto(&self, running: bool, now_ms: i64) -> AutoLessonsDto {
        let next_ms = match due(self, now_ms, running) {
            Due::NotYet { next_ms } => Some(next_ms),
            _ => None,
        };
        AutoLessonsDto {
            enabled: self.enabled,
            running,
            last_run_ms: self.last_finished_ms.max(self.last_attempt_ms),
            last_check_ms: self.last_check_ms,
            last_outcome: self.last_outcome.clone(),
            next_ms,
        }
    }
}

/// Change the stored setting with `change`, under one hold of `db`.
fn update(
    db: &Mutex<Connection>,
    change: impl FnOnce(&mut AutoLessons),
) -> Result<AutoLessons, String> {
    let conn = db.lock().map_err(|_| "state lock poisoned")?;
    let mut settings = AutoLessons::load(&conn);
    change(&mut settings);
    settings.save(&conn)?;
    Ok(settings)
}

/// Remember `agent` as the one the person last ran Find lessons with.
pub(crate) fn remember_agent(db: &Mutex<Connection>, agent: Agent) -> Result<(), String> {
    update(db, |s| s.agent = Some(agent.id().to_owned())).map(|_| ())
}

/// Whether an automatic run may look for new corrections now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Due {
    /// The person turned it off.
    Disabled,
    /// A run, manual or automatic, is going.
    Busy,
    /// Too soon after the last finished run, the last failed one, or the last
    /// empty check.
    NotYet { next_ms: i64 },
    /// Look for new corrections, and run when there are any.
    CheckForNew,
}

/// Decide, without mining, whether to look for new corrections at `now_ms`:
/// at most one finished run a day, a retry an hour after a run that did not
/// finish, none after [`MAX_FAILURES`] in a row until a day after the first,
/// and at most one look an hour.
pub(crate) fn due(settings: &AutoLessons, now_ms: i64, running: bool) -> Due {
    if !settings.enabled {
        return Due::Disabled;
    }
    if running {
        return Due::Busy;
    }
    let plus = |t: Option<i64>, ms: i64| t.map(|t| t.saturating_add(ms));
    let after_finish = plus(settings.last_finished_ms, DAY_MS);
    let failing = settings.consecutive_failures > 0;
    let after_retry = plus(settings.last_attempt_ms.filter(|_| failing), HOUR_MS);
    let after_streak = plus(
        settings
            .failing_since_ms
            .or(settings.last_attempt_ms)
            .filter(|_| settings.consecutive_failures >= MAX_FAILURES),
        DAY_MS,
    );
    let after_check = plus(settings.last_check_ms, HOUR_MS);
    match after_finish
        .max(after_retry)
        .max(after_streak)
        .max(after_check)
    {
        Some(next_ms) if now_ms < next_ms => Due::NotYet { next_ms },
        _ => Due::CheckForNew,
    }
}

/// Who holds the [`LessonsLock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Runner {
    Manual = 1,
    Auto = 2,
}

/// Held by whichever Find lessons run is going, so a manual and an automatic
/// run never overlap.
#[derive(Debug, Clone, Default)]
pub struct LessonsLock(Arc<AtomicU8>);

/// Lets go of the [`LessonsLock`] when dropped.
#[derive(Debug)]
pub struct LessonsGuard(Arc<AtomicU8>);

impl Drop for LessonsGuard {
    fn drop(&mut self) {
        self.0.store(0, Ordering::Release);
    }
}

impl LessonsLock {
    /// Take the lock for `runner`, or `None` while a run holds it.
    pub(crate) fn try_take(&self, runner: Runner) -> Option<LessonsGuard> {
        self.0
            .compare_exchange(0, runner as u8, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| LessonsGuard(Arc::clone(&self.0)))
    }

    pub(crate) fn held(&self) -> bool {
        self.0.load(Ordering::Acquire) != 0
    }

    pub(crate) fn auto_running(&self) -> bool {
        self.0.load(Ordering::Acquire) == Runner::Auto as u8
    }
}

/// On an ingest event at `now_ms`: decide whether a look is [`due`], and when
/// it is, take the lock, stamp `last_check_ms`, and hand the lock to `spawn`,
/// which starts [`work`] on a thread of its own. Mines nothing itself.
pub(crate) fn poke(
    db: &Mutex<Connection>,
    lock: &LessonsLock,
    now_ms: i64,
    spawn: impl FnOnce(LessonsGuard),
) -> Due {
    // Never wait on a UI query from the ingest thread; the next event looks again.
    let Ok(conn) = db.try_lock() else {
        return Due::Busy;
    };
    let mut settings = AutoLessons::load(&conn);
    let due = due(&settings, now_ms, lock.held());
    if due != Due::CheckForNew {
        return due;
    }
    let Some(guard) = lock.try_take(Runner::Auto) else {
        return Due::Busy;
    };
    settings.last_check_ms = Some(now_ms);
    // Unsaved, the stamp would not hold back the next event: do not look.
    if settings.save(&conn).is_err() {
        return Due::Busy;
    }
    drop(conn);
    spawn(guard);
    due
}

/// How a look ended, for the run counts in [`AutoLessons`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// No run was started: mining failed or found nothing new.
    Looked,
    /// There was something new, but no agent could start.
    NotStarted,
    /// The run finished.
    Finished,
    /// The run failed or timed out.
    Failed,
}

/// One automatic look, on its own thread: mine with `pending`; when anything
/// is new, `choose` an agent from the one last used, count an attempt, and
/// `run` it, which returns how many lessons it filed. A run that returns `Ok`
/// finished; any other end, or no agent to choose, is a failed attempt. Every
/// end is stored as `last_outcome`. `notify` hears the setting when a run
/// starts (`true`) and when the look ends (`false`).
pub(crate) fn work(
    db: &Mutex<Connection>,
    now_ms: impl Fn() -> i64,
    pending: impl FnOnce() -> Result<BTreeSet<CorrectionId>, String>,
    choose: impl FnOnce(Option<Agent>) -> Option<Agent>,
    run: impl FnOnce(Agent, &BTreeSet<CorrectionId>) -> Result<usize, String>,
    notify: impl Fn(&AutoLessons, bool),
) -> Result<AutoLessons, String> {
    let finish = |outcome: String, end: End| {
        let outcome: String = outcome.chars().take(OUTCOME_CHARS).collect();
        let settings = update(db, |s| {
            let t = now_ms();
            match end {
                End::Looked => {}
                End::NotStarted => s.begin_attempt(t),
                End::Finished => s.finish_attempt(t),
                End::Failed => s.fail_attempt(t),
            }
            s.last_outcome = Some(outcome);
        })?;
        notify(&settings, false);
        Ok(settings)
    };
    let pending = match pending() {
        Ok(pending) => pending,
        Err(e) => {
            return finish(format!("Could not read your corrections: {e}"), End::Looked);
        }
    };
    if pending.is_empty() {
        return finish("No new corrections.".to_owned(), End::Looked);
    }
    let last = {
        let conn = db.lock().map_err(|_| "state lock poisoned")?;
        AutoLessons::load(&conn)
            .agent
            .as_deref()
            .and_then(Agent::parse)
    };
    let Some(agent) = choose(last) else {
        return finish(
            "Not run: sign in to Claude Code or Codex.".to_owned(),
            End::NotStarted,
        );
    };
    let started = update(db, |s| s.begin_attempt(now_ms()))?;
    notify(&started, true);
    let label = agent.label();
    match run(agent, &pending) {
        Ok(filed) => {
            let n = pending.len();
            let corrections = if n == 1 { "correction" } else { "corrections" };
            let lessons = if filed == 1 { "lesson" } else { "lessons" };
            finish(
                format!("{label} read {n} new {corrections} and filed {filed} {lessons}."),
                End::Finished,
            )
        }
        Err(e) => finish(format!("{label} failed: {e}"), End::Failed),
    }
}

/// How many proposals wait in `vault`'s inbox.
fn waiting(vault: Option<&Vault>) -> usize {
    vault
        .and_then(|v| review::inbox(v).ok())
        .map_or(0, |inbox| inbox.proposals.len())
}

/// The automatic run of `agent` on `pending`: built by `prepare` with Sonnet
/// for Claude Code, recorded in `vault`'s ledger as `run_id`. Returns how many
/// lessons it filed.
pub(crate) fn run_auto(
    agent: Agent,
    pending: &BTreeSet<CorrectionId>,
    vault: Option<&Vault>,
    run_id: &str,
    prepare: impl FnOnce(Agent, Option<&str>) -> Result<Prepared, String>,
) -> Result<usize, String> {
    let model = (agent == Agent::Claude).then_some(CLAUDE_MODEL);
    let prepared = prepare(agent, model)?;
    let before = waiting(vault);
    prepared.run_on(pending, vault, run_id)?;
    Ok(waiting(vault).saturating_sub(before))
}

/// Handle to the thread that runs [`check`] on a timer, so a run that becomes
/// due when nothing is being ingested still starts. Dropping it stops the
/// thread and joins it, like the ingest worker's handle.
pub struct DueTimer {
    tx: Option<Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl DueTimer {
    /// Call `tick` after `first`, then every `every`, until shut down. The
    /// wait is a plain thread timeout: it holds no power assertion and sets
    /// no wake timer, so it neither keeps the Mac awake nor wakes it; a tick
    /// missed while asleep comes at most `every` after waking.
    pub(crate) fn spawn(
        first: Duration,
        every: Duration,
        mut tick: impl FnMut() + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<()>();
        let join = std::thread::Builder::new()
            .name("auto-lessons-timer".to_owned())
            .spawn(move || {
                // A stop signal, or a dropped handle, ends the wait at once.
                let mut wait = first;
                while let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(wait) {
                    tick();
                    wait = every;
                }
            })?;
        Ok(Self {
            tx: Some(tx),
            join: Some(join),
        })
    }

    /// Stop the timer and join its thread. A run it already started keeps
    /// going on its own thread, as one an ingest event started does.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for DueTimer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The [`DueTimer`] of the running app: [`check`] after [`FIRST_TICK`], then
/// every [`TICK`].
pub(crate) fn start_timer(app: &AppHandle) -> Option<DueTimer> {
    let app = app.clone();
    match DueTimer::spawn(FIRST_TICK, TICK, move || check(&app)) {
        Ok(timer) => Some(timer),
        Err(e) => {
            eprintln!("warning: automatic Find lessons timer could not start: {e}");
            None
        }
    }
}

/// Called by the ingest worker after each ingested source and each finished
/// scan, and by the [`DueTimer`]. Cheap: reads one setting, and starts a
/// thread only when a look is due.
pub(crate) fn check(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let app = app.clone();
    poke(
        &state.db,
        &state.lessons,
        lore_core::now_ms(),
        move |guard| {
            let started = std::thread::Builder::new()
                .name("auto-lessons".to_owned())
                .spawn(move || {
                    let _guard = guard;
                    if let Err(e) = look(&app) {
                        eprintln!("warning: automatic Find lessons stopped: {e}");
                    }
                });
            if let Err(e) = started {
                eprintln!("warning: automatic Find lessons could not start: {e}");
            }
        },
    );
}

/// One automatic look in the running app.
fn look(app: &AppHandle) -> Result<AutoLessons, String> {
    let state = app.state::<AppState>();
    let archive = state.archive_dir.clone();
    let vault = lessons::open_vault();
    work(
        &state.db,
        lore_core::now_ms,
        || lessons::pending(&archive, vault.as_ref(), lore_core::now_ms()),
        |last| lessons::pick(last, lessons::ready),
        |agent, pending| {
            let run_id = lessons::run_id(agent, lore_core::now_ms());
            run_auto(agent, pending, vault.as_ref(), &run_id, |agent, model| {
                Prepared::locate(agent, &archive, model)
            })
        },
        |settings, running| {
            let _ = app.emit(EVENT, settings.dto(running, lore_core::now_ms()));
        },
    )
}

/// The automatic run's setting and what it last did.
#[tauri::command]
pub fn get_auto_lessons(state: State<'_, AppState>) -> Result<AutoLessonsDto, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    Ok(AutoLessons::load(&conn).dto(state.lessons.auto_running(), lore_core::now_ms()))
}

/// Turn the automatic run on or off.
#[tauri::command]
pub fn set_auto_lessons(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<AutoLessonsDto, String> {
    let settings = update(&state.db, |s| s.enabled = enabled)?;
    Ok(settings.dto(state.lessons.auto_running(), lore_core::now_ms()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicUsize;

    use lore_memory::considered;

    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const MINUTE: i64 = 60 * 1000;

    fn at(last_finished_ms: Option<i64>, last_check_ms: Option<i64>) -> AutoLessons {
        AutoLessons {
            last_finished_ms,
            last_check_ms,
            ..AutoLessons::default()
        }
    }

    /// `failures` attempts in a row did not finish, the first at `since` and
    /// the last at `last`; each looked when it started.
    fn failing(failures: u32, since: i64, last: i64) -> AutoLessons {
        AutoLessons {
            last_attempt_ms: Some(last),
            consecutive_failures: failures,
            failing_since_ms: Some(since),
            last_check_ms: Some(last),
            ..AutoLessons::default()
        }
    }

    #[test]
    fn a_failed_run_is_retried_after_an_hour() {
        let failed = NOW - 10 * MINUTE;
        let s = failing(1, failed, failed);
        assert_eq!(
            due(&s, NOW, false),
            Due::NotYet {
                next_ms: failed + HOUR_MS
            }
        );
        assert_eq!(
            due(&s, failed + HOUR_MS - 1, false),
            Due::NotYet {
                next_ms: failed + HOUR_MS
            }
        );
        assert_eq!(due(&s, failed + HOUR_MS, false), Due::CheckForNew);
        // A run that timed out 20 minutes after it started waits an hour from
        // the failure, not from the start.
        let started = NOW - 30 * MINUTE;
        let s = AutoLessons {
            last_check_ms: Some(started),
            ..failing(1, started, started + 20 * MINUTE)
        };
        assert_eq!(
            due(&s, started + HOUR_MS, false),
            Due::NotYet {
                next_ms: started + 80 * MINUTE
            }
        );
        assert_eq!(due(&s, started + 80 * MINUTE, false), Due::CheckForNew);
        // A second failure in a row is retried the same way.
        let s = failing(2, NOW - 2 * HOUR_MS, NOW - HOUR_MS);
        assert_eq!(due(&s, NOW, false), Due::CheckForNew);
        // Busy and off still win.
        assert_eq!(due(&s, NOW, true), Due::Busy);
        let off = AutoLessons {
            enabled: false,
            ..s
        };
        assert_eq!(due(&off, NOW, false), Due::Disabled);
    }

    #[test]
    fn after_three_failures_in_a_row_no_retry_until_a_day_after_the_first() {
        let first = NOW;
        let s = failing(MAX_FAILURES, first, first + 2 * HOUR_MS);
        for later in [
            first + 3 * HOUR_MS,
            first + 12 * HOUR_MS,
            first + DAY_MS - 1,
        ] {
            assert_eq!(
                due(&s, later, false),
                Due::NotYet {
                    next_ms: first + DAY_MS
                }
            );
        }
        assert_eq!(due(&s, first + DAY_MS, false), Due::CheckForNew);
        // More than the cap counts the same.
        let s = failing(MAX_FAILURES + 2, first, first + 4 * HOUR_MS);
        assert_eq!(
            due(&s, first + 5 * HOUR_MS, false),
            Due::NotYet {
                next_ms: first + DAY_MS
            }
        );
        // A last failure close to the day's end still waits its hour.
        let s = failing(MAX_FAILURES, first, first + DAY_MS - MINUTE);
        assert_eq!(
            due(&s, first + DAY_MS, false),
            Due::NotYet {
                next_ms: first + DAY_MS - MINUTE + HOUR_MS
            }
        );
    }

    #[test]
    fn attempts_count_failures_in_a_day_and_a_finish_clears_them() {
        let mut s = AutoLessons::default();
        let mut t = NOW;
        for n in 1..=MAX_FAILURES {
            s.begin_attempt(t);
            s.fail_attempt(t + MINUTE);
            assert_eq!(s.consecutive_failures, n);
            assert_eq!(s.failing_since_ms, Some(NOW));
            assert_eq!(s.last_attempt_ms, Some(t + MINUTE));
            t += HOUR_MS + MINUTE;
        }
        s.last_check_ms = Some(t - HOUR_MS);
        assert_eq!(
            due(&s, t, false),
            Due::NotYet {
                next_ms: NOW + DAY_MS
            }
        );
        // A day after the first, the count starts over: an hour's retry again.
        let next_day = NOW + DAY_MS;
        assert_eq!(due(&s, next_day, false), Due::CheckForNew);
        s.begin_attempt(next_day);
        assert_eq!(s.consecutive_failures, 1);
        assert_eq!(s.failing_since_ms, Some(next_day));
        s.fail_attempt(next_day);
        s.last_check_ms = Some(next_day);
        assert_eq!(due(&s, next_day + HOUR_MS, false), Due::CheckForNew);
        // A finished run clears the count and holds the next for a day.
        s.begin_attempt(next_day + HOUR_MS);
        s.finish_attempt(next_day + 2 * HOUR_MS);
        s.last_check_ms = Some(next_day + HOUR_MS);
        assert_eq!(s.consecutive_failures, 0);
        assert_eq!(s.failing_since_ms, None);
        assert_eq!(
            due(&s, next_day + 3 * HOUR_MS, false),
            Due::NotYet {
                next_ms: next_day + 2 * HOUR_MS + DAY_MS
            }
        );
        assert_eq!(
            due(&s, next_day + 2 * HOUR_MS + DAY_MS, false),
            Due::CheckForNew
        );
    }

    #[test]
    fn an_old_last_run_counts_as_finished_only_when_its_outcome_says_so() {
        let db = lore_core::storage::open_in_memory().unwrap();
        let stored = |outcome: &str| {
            let json =
                serde_json::json!({"enabled": true, "last_run_ms": NOW, "last_outcome": outcome});
            lore_core::settings::set(&db, KEY, &json.to_string()).unwrap();
            AutoLessons::load(&db)
        };
        let ok = stored("Claude Code read 75 new corrections and filed 2 lessons.");
        assert_eq!(ok.last_finished_ms, Some(NOW));
        assert_eq!(ok.last_attempt_ms, None);
        assert_eq!(ok.consecutive_failures, 0);
        assert_eq!(
            due(&ok, NOW + HOUR_MS, false),
            Due::NotYet {
                next_ms: NOW + DAY_MS
            }
        );
        let one = stored("Codex read 1 new correction and filed 1 lesson.");
        assert_eq!(one.last_finished_ms, Some(NOW));
        for failed in [
            "Claude Code failed: timed out after 20 minutes",
            "Not run: sign in to Claude Code or Codex.",
            "No new corrections.",
        ] {
            let s = stored(failed);
            assert_eq!(s.last_finished_ms, None, "{failed}");
            assert_eq!(s.last_attempt_ms, Some(NOW));
            assert_eq!(s.consecutive_failures, 1);
            assert_eq!(s.failing_since_ms, Some(NOW));
            assert_eq!(
                due(&s, NOW + HOUR_MS - 1, false),
                Due::NotYet {
                    next_ms: NOW + HOUR_MS
                }
            );
            assert_eq!(due(&s, NOW + HOUR_MS, false), Due::CheckForNew);
        }
        // Saved again, the old field is gone and the new ones stay.
        let s = stored("Claude Code failed: timed out");
        s.save(&db).unwrap();
        let raw = lore_core::settings::get(&db, KEY).unwrap().unwrap();
        assert!(!raw.contains("last_run_ms"), "{raw}");
        assert_eq!(AutoLessons::load(&db), s);
        // New fields already set are not overwritten by a stale old one.
        let json = serde_json::json!({"last_run_ms": 5, "last_finished_ms": NOW});
        lore_core::settings::set(&db, KEY, &json.to_string()).unwrap();
        assert_eq!(AutoLessons::load(&db), at(Some(NOW), None));
    }

    #[test]
    fn due_runs_at_most_once_a_day_and_looks_at_most_once_an_hour() {
        let off = AutoLessons {
            enabled: false,
            ..AutoLessons::default()
        };
        assert_eq!(due(&off, NOW, false), Due::Disabled);
        assert_eq!(due(&off, NOW, true), Due::Disabled);
        // On by default, and never run: look now.
        assert!(AutoLessons::default().enabled);
        assert_eq!(due(&at(None, None), NOW, false), Due::CheckForNew);
        assert_eq!(due(&at(None, None), NOW, true), Due::Busy);
        // Ran 23 hours ago: not until a day after that run.
        let ran = NOW - 23 * HOUR_MS;
        assert_eq!(
            due(&at(Some(ran), Some(ran)), NOW, false),
            Due::NotYet {
                next_ms: ran + DAY_MS
            }
        );
        // Ran 25 hours ago: look again.
        let ran = NOW - 25 * HOUR_MS;
        assert_eq!(due(&at(Some(ran), Some(ran)), NOW, false), Due::CheckForNew);
        // Ran exactly a day ago: due.
        let ran = NOW - DAY_MS;
        assert_eq!(due(&at(Some(ran), None), NOW, false), Due::CheckForNew);
        // Looked 30 minutes ago and found nothing new: not within the hour.
        let looked = NOW - 30 * MINUTE;
        assert_eq!(
            due(&at(None, Some(looked)), NOW, false),
            Due::NotYet {
                next_ms: looked + HOUR_MS
            }
        );
        assert_eq!(
            due(&at(Some(NOW - 25 * HOUR_MS), Some(looked)), NOW, false),
            Due::NotYet {
                next_ms: looked + HOUR_MS
            }
        );
        // Looked 61 minutes ago: look again.
        assert_eq!(
            due(&at(None, Some(NOW - 61 * MINUTE)), NOW, false),
            Due::CheckForNew
        );
        // A clock that went back still waits for the later bound.
        assert_eq!(
            due(&at(Some(NOW + MINUTE), None), NOW, false),
            Due::NotYet {
                next_ms: NOW + MINUTE + DAY_MS
            }
        );
    }

    #[test]
    fn an_unset_or_old_setting_reads_as_on() {
        let db = lore_core::storage::open_in_memory().unwrap();
        assert_eq!(AutoLessons::load(&db), AutoLessons::default());
        lore_core::settings::set(&db, KEY, r#"{"last_finished_ms":5}"#).unwrap();
        assert_eq!(AutoLessons::load(&db), at(Some(5), None));
        lore_core::settings::set(&db, KEY, "\"garbage\"").unwrap();
        assert_eq!(AutoLessons::load(&db), AutoLessons::default());
    }

    #[test]
    fn picks_the_agent_last_used_else_claude_code_else_codex() {
        let all = |_| true;
        assert_eq!(lessons::pick(Some(Agent::Codex), all), Some(Agent::Codex));
        assert_eq!(lessons::pick(None, all), Some(Agent::Claude));
        let only_codex = |a| a == Agent::Codex;
        assert_eq!(
            lessons::pick(Some(Agent::Claude), only_codex),
            Some(Agent::Codex)
        );
        assert_eq!(lessons::pick(None, |_| false), None);
    }

    /// A fake `claude` that appends its arguments and a `--` line to `argv`
    /// and reports success.
    fn fake_claude(dir: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("claude");
        let argv = dir.join("argv.txt");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' \"$@\" >> '{}'\necho -- >> '{}'\n\
                 printf '%s' '{{\"result\":\"Filed: 0 rules\",\"is_error\":false}}'\n",
                argv.display(),
                argv.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, argv)
    }

    fn ids(texts: &[&str]) -> BTreeSet<CorrectionId> {
        texts
            .iter()
            .map(|t| CorrectionId::parse(t).unwrap())
            .collect()
    }

    /// A Lore app's pieces: settings, the lock, a vault, a fake `claude`.
    struct World {
        dir: tempfile::TempDir,
        db: Mutex<Connection>,
        lock: LessonsLock,
        vault: Vault,
        bin: PathBuf,
        argv: PathBuf,
        mined: AtomicUsize,
    }

    impl World {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let (vault, _) = Vault::init(&dir.path().join("vault")).unwrap();
            let (bin, argv) = fake_claude(dir.path());
            Self {
                db: Mutex::new(lore_core::storage::open_in_memory().unwrap()),
                lock: LessonsLock::default(),
                vault,
                bin,
                argv,
                mined: AtomicUsize::new(0),
                dir,
            }
        }

        fn settings(&self) -> AutoLessons {
            AutoLessons::load(&self.db.lock().unwrap())
        }

        /// The runs the fake agent saw, each as its argument list.
        fn runs(&self) -> Vec<Vec<String>> {
            let text = std::fs::read_to_string(&self.argv).unwrap_or_default();
            let mut runs = vec![];
            let mut run = vec![];
            for line in text.lines() {
                if line == "--" {
                    runs.push(std::mem::take(&mut run));
                } else {
                    run.push(line.to_owned());
                }
            }
            runs
        }

        /// An ingest event at `now`, with the look run right here instead of
        /// on a thread. `new` is what mining finds, less what the ledger holds.
        fn event(&self, now: i64, new: &BTreeSet<CorrectionId>) -> Due {
            self.event_with(now, new, |_| {})
        }

        fn event_with(
            &self,
            now: i64,
            new: &BTreeSet<CorrectionId>,
            during_run: impl Fn(&LessonsLock),
        ) -> Due {
            poke(&self.db, &self.lock, now, |guard| {
                let _guard = guard;
                work(
                    &self.db,
                    || now,
                    || {
                        self.mined.fetch_add(1, Ordering::SeqCst);
                        let seen = considered::considered(&self.vault).unwrap();
                        Ok(new.difference(&seen).cloned().collect())
                    },
                    |last| lessons::pick(last, |_| true),
                    |agent, pending| {
                        during_run(&self.lock);
                        run_auto(
                            agent,
                            pending,
                            Some(&self.vault),
                            "auto-1",
                            |agent, model| {
                                Prepared::new(
                                    agent,
                                    self.bin.clone(),
                                    self.dir.path().to_owned(),
                                    "/usr/bin:/bin".into(),
                                    Path::new("/b/lorectl"),
                                    self.dir.path(),
                                    self.dir.path(),
                                    model,
                                )
                            },
                        )
                    },
                    |_, _| {},
                )
                .unwrap();
            })
        }
    }

    #[test]
    fn new_corrections_start_exactly_one_sonnet_run() {
        let world = World::new();
        let new = ids(&["cr-0123456789abcdef", "cr-aaaaaaaaaaaaaaaa"]);
        assert_eq!(world.event(NOW, &new), Due::CheckForNew);
        let runs = world.runs();
        assert_eq!(runs.len(), 1, "{runs:?}");
        let model = runs[0].iter().position(|a| a == "--model").unwrap();
        assert_eq!(runs[0][model + 1], "sonnet");
        assert_eq!(runs[0][0], "-p");
        let settings = world.settings();
        assert_eq!(settings.last_finished_ms, Some(NOW));
        assert_eq!(settings.consecutive_failures, 0);
        assert_eq!(settings.last_check_ms, Some(NOW));
        assert_eq!(
            settings.last_outcome.as_deref(),
            Some("Claude Code read 2 new corrections and filed 0 lessons.")
        );
        assert_eq!(considered::considered(&world.vault).unwrap(), new);
        assert!(!world.lock.held());

        // More events the same day start nothing, and mine nothing.
        let more = ids(&["cr-bbbbbbbbbbbbbbbb"]);
        for later in [NOW + MINUTE, NOW + 2 * HOUR_MS, NOW + 23 * HOUR_MS] {
            assert!(matches!(world.event(later, &more), Due::NotYet { .. }));
        }
        assert_eq!(world.mined.load(Ordering::SeqCst), 1);
        assert_eq!(world.runs().len(), 1);
        // A day later, the new one is read.
        assert_eq!(world.event(NOW + DAY_MS, &more), Due::CheckForNew);
        assert_eq!(world.runs().len(), 2);
    }

    #[test]
    fn with_nothing_new_no_agent_starts_and_the_look_waits_an_hour() {
        let world = World::new();
        let none = BTreeSet::new();
        assert_eq!(world.event(NOW, &none), Due::CheckForNew);
        assert!(world.runs().is_empty());
        let settings = world.settings();
        assert_eq!(settings.last_check_ms, Some(NOW));
        assert_eq!(settings.last_finished_ms, None);
        assert_eq!(settings.last_attempt_ms, None);
        assert_eq!(settings.consecutive_failures, 0);
        assert_eq!(
            settings.last_outcome.as_deref(),
            Some("No new corrections.")
        );
        assert_eq!(world.mined.load(Ordering::SeqCst), 1);

        // A stream of watcher events within the hour does not mine again.
        for later in [NOW + 1, NOW + 10 * MINUTE, NOW + 59 * MINUTE] {
            assert_eq!(
                world.event(later, &none),
                Due::NotYet {
                    next_ms: NOW + HOUR_MS
                }
            );
        }
        assert_eq!(world.mined.load(Ordering::SeqCst), 1);
        assert_eq!(world.event(NOW + HOUR_MS, &none), Due::CheckForNew);
        assert_eq!(world.mined.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_turned_off_run_never_looks() {
        let world = World::new();
        update(&world.db, |s| s.enabled = false).unwrap();
        assert_eq!(
            world.event(NOW, &ids(&["cr-0123456789abcdef"])),
            Due::Disabled
        );
        assert_eq!(world.mined.load(Ordering::SeqCst), 0);
        assert!(world.runs().is_empty());
    }

    #[test]
    fn manual_and_automatic_runs_never_overlap() {
        let world = World::new();
        let new = ids(&["cr-0123456789abcdef"]);
        // A manual run is going: an automatic one does not even look.
        let manual = world.lock.try_take(Runner::Manual).unwrap();
        assert_eq!(world.event(NOW, &new), Due::Busy);
        assert_eq!(world.mined.load(Ordering::SeqCst), 0);
        assert_eq!(world.settings().last_check_ms, None);
        drop(manual);

        // An automatic run is going: the button is refused.
        let refused = AtomicUsize::new(0);
        world.event_with(NOW, &new, |lock| {
            assert!(lock.auto_running());
            if lock.try_take(Runner::Manual).is_none() {
                refused.fetch_add(1, Ordering::SeqCst);
            }
        });
        assert_eq!(refused.load(Ordering::SeqCst), 1);
        assert_eq!(world.runs().len(), 1);
        // Once it ends, the button works again.
        assert!(world.lock.try_take(Runner::Manual).is_some());
    }

    /// Run `n` ticks of a [`DueTimer`] on `world`, the `i`th at `NOW + i *
    /// TICK`, with nothing ingested; return `world` and what each decided.
    fn ticks(world: World, n: usize, new: &BTreeSet<CorrectionId>) -> (World, Vec<Due>) {
        let (tx, rx) = mpsc::channel();
        let world = Arc::new(Mutex::new(world));
        let (w, new) = (Arc::clone(&world), new.clone());
        let mut i = 0_i64;
        let tick_ms = i64::try_from(TICK.as_millis()).unwrap();
        let timer = DueTimer::spawn(
            Duration::from_millis(1),
            Duration::from_millis(1),
            move || {
                let _ = tx.send(w.lock().unwrap().event(NOW + i * tick_ms, &new));
                i += 1;
            },
        )
        .unwrap();
        let seen = (0..n)
            .map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect();
        timer.shutdown();
        let world = Arc::try_unwrap(world).ok().unwrap().into_inner().unwrap();
        (world, seen)
    }

    #[test]
    fn a_due_run_starts_on_a_timer_tick_with_nothing_ingested() {
        let world = World::new();
        // The last run was a day ago; the corrections since are waiting.
        update(&world.db, |s| s.last_finished_ms = Some(NOW - DAY_MS)).unwrap();
        let new = ids(&["cr-0123456789abcdef", "cr-aaaaaaaaaaaaaaaa"]);
        let (world, seen) = ticks(world, 6, &new);
        assert_eq!(seen[0], Due::CheckForNew);
        assert!(
            seen[1..].iter().all(|d| matches!(d, Due::NotYet { .. })),
            "{seen:?}"
        );
        assert_eq!(world.runs().len(), 1);
        assert_eq!(world.mined.load(Ordering::SeqCst), 1);
        assert_eq!(world.settings().last_finished_ms, Some(NOW));
        assert_eq!(considered::considered(&world.vault).unwrap(), new);
        assert!(!world.lock.held());
    }

    #[test]
    fn ticks_that_are_not_due_mine_nothing() {
        let world = World::new();
        update(&world.db, |s| s.last_finished_ms = Some(NOW - HOUR_MS)).unwrap();
        let (world, seen) = ticks(world, 5, &ids(&["cr-0123456789abcdef"]));
        assert!(
            seen.iter().all(|d| matches!(d, Due::NotYet { .. })),
            "{seen:?}"
        );
        assert_eq!(world.mined.load(Ordering::SeqCst), 0);
        assert!(world.runs().is_empty());

        // Turned off: a due tick still never looks.
        update(&world.db, |s| {
            s.enabled = false;
            s.last_finished_ms = None;
        })
        .unwrap();
        let (world, seen) = ticks(world, 2, &BTreeSet::new());
        assert_eq!(seen, [Due::Disabled; 2]);
        assert_eq!(world.mined.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_tick_during_a_manual_run_starts_nothing() {
        let world = World::new();
        let new = ids(&["cr-0123456789abcdef"]);
        let manual = world.lock.try_take(Runner::Manual).unwrap();
        let (world, seen) = ticks(world, 3, &new);
        assert_eq!(seen, [Due::Busy; 3]);
        assert_eq!(world.mined.load(Ordering::SeqCst), 0);
        assert!(world.runs().is_empty());
        assert_eq!(world.settings().last_check_ms, None);
        drop(manual);
        // Once it ends, the next tick runs.
        let (world, seen) = ticks(world, 1, &new);
        assert_eq!(seen, [Due::CheckForNew]);
        assert_eq!(world.runs().len(), 1);
    }

    #[test]
    fn the_timer_stops_promptly_and_waits_for_its_first_tick() {
        let ticked = Arc::new(AtomicUsize::new(0));
        let t = Arc::clone(&ticked);
        let timer = DueTimer::spawn(TICK, TICK, move || {
            t.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        let started = std::time::Instant::now();
        timer.shutdown();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(ticked.load(Ordering::SeqCst), 0);
        // Dropping it stops it too.
        drop(DueTimer::spawn(TICK, TICK, || {}).unwrap());
    }

    #[test]
    fn the_button_remembers_its_agent_for_the_automatic_run() {
        let world = World::new();
        remember_agent(&world.db, Agent::Codex).unwrap();
        let chose = Mutex::new(None);
        work(
            &world.db,
            || NOW,
            || Ok(ids(&["cr-0123456789abcdef"])),
            |last| {
                *chose.lock().unwrap() = Some(last);
                None
            },
            |_, _| panic!("no agent was ready"),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(*chose.lock().unwrap(), Some(Some(Agent::Codex)));
        let settings = world.settings();
        assert_eq!(settings.last_finished_ms, None);
        assert_eq!(
            settings.last_outcome.as_deref(),
            Some("Not run: sign in to Claude Code or Codex.")
        );
        // Not starting is a failed attempt: tried again in an hour.
        assert_eq!(settings.last_attempt_ms, Some(NOW));
        assert_eq!(settings.consecutive_failures, 1);
    }

    /// One automatic look at `now` on `world` whose run ends with `result`.
    fn look_at(world: &World, now: i64, result: Result<usize, String>) -> Due {
        let result = Mutex::new(Some(result));
        poke(&world.db, &world.lock, now, |guard| {
            let _guard = guard;
            work(
                &world.db,
                || now,
                || Ok(ids(&["cr-0123456789abcdef"])),
                |_| Some(Agent::Claude),
                |_, _| result.lock().unwrap().take().unwrap(),
                |_, _| {},
            )
            .unwrap();
        })
    }

    #[test]
    fn a_failed_run_does_not_use_up_the_day() {
        let world = World::new();
        let fail = || Err("timed out after 20 minutes".to_owned());
        assert_eq!(look_at(&world, NOW, fail()), Due::CheckForNew);
        let s = world.settings();
        assert_eq!(s.last_finished_ms, None);
        assert_eq!((s.last_attempt_ms, s.consecutive_failures), (Some(NOW), 1));
        assert_eq!(
            s.last_outcome.as_deref(),
            Some("Claude Code failed: timed out after 20 minutes")
        );
        // Not within the hour; an hour later, tried again.
        assert!(matches!(
            look_at(&world, NOW + 59 * MINUTE, fail()),
            Due::NotYet { .. }
        ));
        assert_eq!(look_at(&world, NOW + HOUR_MS, fail()), Due::CheckForNew);
        assert_eq!(look_at(&world, NOW + 2 * HOUR_MS, fail()), Due::CheckForNew);
        assert_eq!(world.settings().consecutive_failures, MAX_FAILURES);
        // Three in a row: none until a day after the first.
        for later in [3, 10, 23] {
            assert_eq!(
                look_at(&world, NOW + later * HOUR_MS, fail()),
                Due::NotYet {
                    next_ms: NOW + DAY_MS
                }
            );
        }
        // A manual run does not touch the count.
        remember_agent(&world.db, Agent::Codex).unwrap();
        assert_eq!(world.settings().consecutive_failures, MAX_FAILURES);
        assert!(matches!(
            due(&world.settings(), NOW + 23 * HOUR_MS, false),
            Due::NotYet { .. }
        ));
        // The next day it runs, finishes, and holds the next for a day.
        assert_eq!(look_at(&world, NOW + DAY_MS, Ok(1)), Due::CheckForNew);
        let s = world.settings();
        assert_eq!(s.last_finished_ms, Some(NOW + DAY_MS));
        assert_eq!((s.consecutive_failures, s.failing_since_ms), (0, None));
        assert_eq!(
            s.dto(false, NOW + DAY_MS + HOUR_MS).next_ms,
            Some(NOW + 2 * DAY_MS)
        );
        assert!(matches!(
            look_at(&world, NOW + 2 * DAY_MS - 1, Ok(1)),
            Due::NotYet { .. }
        ));
        assert_eq!(look_at(&world, NOW + 2 * DAY_MS, Ok(1)), Due::CheckForNew);
    }
}
