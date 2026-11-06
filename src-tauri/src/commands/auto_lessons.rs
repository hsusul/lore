//! Find lessons on its own: soon after the person corrects their agent, the
//! app runs the same pinned Find lessons run the button starts, at most once a
//! day and only when there are corrections no earlier run has read.
//!
//! Ingest events ([`on_ingest`]) only look at one setting row. Mining and the
//! run happen on a thread of their own, so ingest never waits on either. One
//! [`LessonsLock`] is held by whichever run is going, manual or automatic, so
//! two never overlap.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

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
/// At most one automatic run starts in this long.
pub(crate) const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// A check that found nothing new is not repeated for this long, so a stream
/// of watcher events does not mine the archive again and again.
pub(crate) const HOUR_MS: i64 = 60 * 60 * 1000;
/// An automatic Claude Code run uses Sonnet, not the person's default model,
/// to keep a run they did not ask for cheap. Codex keeps its own default.
const CLAUDE_MODEL: &str = "sonnet";
/// An outcome is one line in Settings; a longer error is cut.
const OUTCOME_CHARS: usize = 200;

/// The automatic run's setting and what it last did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AutoLessons {
    /// On unless the person turned it off.
    pub enabled: bool,
    /// When an automatic run last started an agent.
    pub last_run_ms: Option<i64>,
    /// When Lore last looked for new corrections on its own.
    pub last_check_ms: Option<i64>,
    /// What that look led to, in a short sentence.
    pub last_outcome: Option<String>,
    /// The agent the person last ran Find lessons with, as `find_lessons` takes it.
    pub agent: Option<String>,
}

impl Default for AutoLessons {
    fn default() -> Self {
        Self {
            enabled: true,
            last_run_ms: None,
            last_check_ms: None,
            last_outcome: None,
            agent: None,
        }
    }
}

impl AutoLessons {
    /// The stored setting; the defaults when it is unset or unreadable.
    fn load(conn: &Connection) -> Self {
        lore_core::settings::get(conn, KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save(&self, conn: &Connection) -> Result<(), String> {
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        lore_core::settings::set(conn, KEY, &json).map_err(|e| e.to_string())
    }

    fn dto(&self, running: bool) -> AutoLessonsDto {
        AutoLessonsDto {
            enabled: self.enabled,
            running,
            last_run_ms: self.last_run_ms,
            last_check_ms: self.last_check_ms,
            last_outcome: self.last_outcome.clone(),
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
    /// Too soon after the last run or the last empty check.
    NotYet { next_ms: i64 },
    /// Look for new corrections, and run when there are any.
    CheckForNew,
}

/// Decide, without mining, whether to look for new corrections at `now_ms`:
/// at most one run a day, and at most one look an hour.
pub(crate) fn due(settings: &AutoLessons, now_ms: i64, running: bool) -> Due {
    if !settings.enabled {
        return Due::Disabled;
    }
    if running {
        return Due::Busy;
    }
    let after_run = settings.last_run_ms.map(|t| t.saturating_add(DAY_MS));
    let after_check = settings.last_check_ms.map(|t| t.saturating_add(HOUR_MS));
    match after_run.max(after_check) {
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

/// One automatic look, on its own thread: mine with `pending`; when anything
/// is new, `choose` an agent from the one last used, stamp `last_run_ms`, and
/// `run` it, which returns how many lessons it filed. Every end is stored as
/// `last_outcome`. `notify` hears the setting when a run starts (`true`) and
/// when the look ends (`false`).
pub(crate) fn work(
    db: &Mutex<Connection>,
    now_ms: impl Fn() -> i64,
    pending: impl FnOnce() -> Result<BTreeSet<CorrectionId>, String>,
    choose: impl FnOnce(Option<Agent>) -> Option<Agent>,
    run: impl FnOnce(Agent, &BTreeSet<CorrectionId>) -> Result<usize, String>,
    notify: impl Fn(&AutoLessons, bool),
) -> Result<AutoLessons, String> {
    let finish = |outcome: String| {
        let outcome: String = outcome.chars().take(OUTCOME_CHARS).collect();
        let settings = update(db, |s| s.last_outcome = Some(outcome))?;
        notify(&settings, false);
        Ok(settings)
    };
    let pending = match pending() {
        Ok(pending) => pending,
        Err(e) => return finish(format!("Could not read your corrections: {e}")),
    };
    if pending.is_empty() {
        return finish("No new corrections.".to_owned());
    }
    let last = {
        let conn = db.lock().map_err(|_| "state lock poisoned")?;
        AutoLessons::load(&conn)
            .agent
            .as_deref()
            .and_then(Agent::parse)
    };
    let Some(agent) = choose(last) else {
        return finish("Not run: sign in to Claude Code or Codex.".to_owned());
    };
    let started = update(db, |s| s.last_run_ms = Some(now_ms()))?;
    notify(&started, true);
    let label = agent.label();
    finish(match run(agent, &pending) {
        Ok(filed) => {
            let n = pending.len();
            let corrections = if n == 1 { "correction" } else { "corrections" };
            let lessons = if filed == 1 { "lesson" } else { "lessons" };
            format!("{label} read {n} new {corrections} and filed {filed} {lessons}.")
        }
        Err(e) => format!("{label} failed: {e}"),
    })
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

/// Called by the ingest worker after each ingested source and each finished
/// scan. Cheap: reads one setting, and starts a thread only when a look is due.
pub(crate) fn on_ingest(app: &AppHandle) {
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
        || lessons::pending(&archive, vault.as_ref()),
        |last| lessons::pick(last, lessons::ready),
        |agent, pending| {
            let run_id = lessons::run_id(agent, lore_core::now_ms());
            run_auto(agent, pending, vault.as_ref(), &run_id, |agent, model| {
                Prepared::locate(agent, &archive, model)
            })
        },
        |settings, running| {
            let _ = app.emit(EVENT, settings.dto(running));
        },
    )
}

/// The automatic run's setting and what it last did.
#[tauri::command]
pub fn get_auto_lessons(state: State<'_, AppState>) -> Result<AutoLessonsDto, String> {
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    Ok(AutoLessons::load(&conn).dto(state.lessons.auto_running()))
}

/// Turn the automatic run on or off.
#[tauri::command]
pub fn set_auto_lessons(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<AutoLessonsDto, String> {
    let settings = update(&state.db, |s| s.enabled = enabled)?;
    Ok(settings.dto(state.lessons.auto_running()))
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

    fn at(last_run_ms: Option<i64>, last_check_ms: Option<i64>) -> AutoLessons {
        AutoLessons {
            last_run_ms,
            last_check_ms,
            ..AutoLessons::default()
        }
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
        lore_core::settings::set(&db, KEY, r#"{"last_run_ms":5}"#).unwrap();
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
        assert_eq!(settings.last_run_ms, Some(NOW));
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
        assert_eq!(settings.last_run_ms, None);
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
        assert_eq!(settings.last_run_ms, None);
        assert_eq!(
            settings.last_outcome.as_deref(),
            Some("Not run: sign in to Claude Code or Codex.")
        );
    }
}
