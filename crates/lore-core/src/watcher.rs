//! `notify`/FSEvents watching with deterministic path debouncing.
//!
//! The OS callback only sends events through a standard channel. Callers poll
//! ready paths and decide which adapter/job owns them; no database or UI logic
//! runs inside the callback.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Debug, thiserror::Error)]
pub enum WatcherError {
    #[error("filesystem watcher error")]
    Notify(#[from] notify::Error),
}

pub type Result<T> = std::result::Result<T, WatcherError>;

/// Coalesces repeated notifications until a path has been quiet for the
/// configured interval. `Instant` injection keeps tests deterministic.
#[derive(Debug)]
pub struct PathDebouncer {
    quiet_period: Duration,
    pending: BTreeMap<PathBuf, Instant>,
}

impl PathDebouncer {
    #[must_use]
    pub fn new(quiet_period: Duration) -> Self {
        Self {
            quiet_period,
            pending: BTreeMap::new(),
        }
    }

    pub fn record(&mut self, path: PathBuf, observed_at: Instant) {
        self.pending.insert(path, observed_at);
    }

    /// Drain paths whose most recent event is at least one quiet period old.
    pub fn drain_ready(&mut self, now: Instant) -> Vec<PathBuf> {
        let ready: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, observed_at)| {
                now.saturating_duration_since(**observed_at) >= self.quiet_period
            })
            .map(|(path, _)| path.clone())
            .collect();
        for path in &ready {
            self.pending.remove(path);
        }
        ready
    }

    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

/// Non-blocking result of draining OS notifications once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchPoll {
    pub ready_paths: Vec<PathBuf>,
    pub errors: usize,
}

/// Recursive watcher for configured agent roots. Dropping this value stops all
/// underlying OS watches.
pub struct SessionWatcher {
    _watcher: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
    debouncer: PathDebouncer,
}

impl SessionWatcher {
    /// Watch every existing root recursively. Missing roots are ignored because
    /// agent installation/directory creation may happen after app startup; the
    /// app layer can rebuild watches after its next discovery pass.
    pub fn new(roots: &[PathBuf], quiet_period: Duration) -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(sender)?;
        for root in roots.iter().filter(|root| root.is_dir()) {
            watcher.watch(root, RecursiveMode::Recursive)?;
        }
        Ok(Self {
            _watcher: watcher,
            receiver,
            debouncer: PathDebouncer::new(quiet_period),
        })
    }

    /// Drain currently buffered events without blocking and return paths that
    /// have passed the debounce quiet period.
    #[must_use]
    pub fn poll(&mut self) -> WatchPoll {
        self.poll_at(Instant::now())
    }

    fn poll_at(&mut self, now: Instant) -> WatchPoll {
        let mut errors = 0;
        for result in self.receiver.try_iter() {
            match result {
                Ok(event) if should_ingest(&event.kind) => {
                    for path in event.paths.into_iter().filter(|path| is_jsonl(path)) {
                        self.debouncer.record(path, now);
                    }
                }
                Ok(_) => {}
                Err(_) => errors += 1,
            }
        }
        WatchPoll {
            ready_paths: self.debouncer.drain_ready(now),
            errors,
        }
    }
}

fn should_ingest(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
}

fn is_jsonl(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_paths_wait_for_the_latest_quiet_period() {
        let start = Instant::now();
        let path = PathBuf::from("session.jsonl");
        let mut debouncer = PathDebouncer::new(Duration::from_millis(100));
        debouncer.record(path.clone(), start);
        debouncer.record(path.clone(), start + Duration::from_millis(80));

        assert!(debouncer
            .drain_ready(start + Duration::from_millis(100))
            .is_empty());
        assert_eq!(debouncer.pending_len(), 1);
        assert_eq!(
            debouncer.drain_ready(start + Duration::from_millis(180)),
            vec![path]
        );
        assert_eq!(debouncer.pending_len(), 0);
    }

    #[test]
    fn distinct_paths_are_returned_in_stable_order() {
        let start = Instant::now();
        let mut debouncer = PathDebouncer::new(Duration::ZERO);
        debouncer.record(PathBuf::from("z.jsonl"), start);
        debouncer.record(PathBuf::from("a.jsonl"), start);
        assert_eq!(
            debouncer.drain_ready(start),
            vec![PathBuf::from("a.jsonl"), PathBuf::from("z.jsonl")]
        );
    }

    #[test]
    fn path_and_event_filters_match_session_updates() {
        assert!(is_jsonl(Path::new("rollout-test.jsonl")));
        assert!(!is_jsonl(Path::new("state.db")));
        assert!(should_ingest(&EventKind::Any));
        assert!(!should_ingest(&EventKind::Access(
            notify::event::AccessKind::Any
        )));
    }

    #[test]
    fn watcher_accepts_existing_and_missing_roots() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-created");
        let watcher = SessionWatcher::new(
            &[dir.path().to_path_buf(), missing],
            Duration::from_millis(50),
        );
        assert!(watcher.is_ok());
    }

    #[test]
    fn watch_poll_defaults_and_debouncer_clock_skew() {
        let poll = WatchPoll::default();
        assert!(poll.ready_paths.is_empty());
        assert_eq!(poll.errors, 0);
        assert_eq!(poll.clone(), poll);

        // Test clock jumping backward (now < observed_at)
        let start = Instant::now();
        let future = start + Duration::from_secs(10);
        let path = PathBuf::from("future.jsonl");

        let mut debouncer = PathDebouncer::new(Duration::from_millis(100));
        debouncer.record(path.clone(), future);

        // start < future -> saturating_duration_since returns 0 -> not ready
        assert!(debouncer.drain_ready(start).is_empty());
        assert_eq!(debouncer.pending_len(), 1);

        // future + 150ms -> ready
        assert_eq!(
            debouncer.drain_ready(future + Duration::from_millis(150)),
            vec![path]
        );
        assert_eq!(debouncer.pending_len(), 0);
    }
}
