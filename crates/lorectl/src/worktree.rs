//! Reading a person's working tree with a time limit.
//!
//! Opening a file under a folder macOS protects (Desktop, Documents,
//! Downloads, a network volume) can block until the person answers a privacy
//! prompt, and nobody answers one during an unattended run. A read that never
//! returns would stall the MCP server, whose one thread answers every call,
//! or a hook, which stalls the agent itself.
//!
//! So repository discovery, anchor pinning, and file reads in a working tree
//! run on a worker thread, and the caller gives up after `limit` and goes on
//! without them. A worker that is stuck is left to finish on its own, once:
//! while it is, more work on the same folder is refused at once rather than
//! starting another thread, and at most [`MAX_STUCK`] are ever left.

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex, PoisonError};
use std::time::Duration;

use lore_memory::repo::RepoContext;

/// Workers left running past their limit, at most.
const MAX_STUCK: usize = 8;

/// The folders with a worker running, each with the worker's number.
static RUNNING: Mutex<(u64, Vec<(u64, PathBuf)>)> = Mutex::new((0, Vec::new()));

/// The work did not finish within its limit, or an earlier worker on the same
/// folder still has not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedOut;

/// `work`, run on a worker thread, when it finishes within `limit`. `folder`
/// is the working tree it reads, so a folder that is already stuck is not
/// tried again until its worker returns.
pub fn within<T, F>(folder: &Path, limit: Duration, work: F) -> Result<T, TimedOut>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let id = {
        let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
        let (next, workers) = &mut *running;
        if workers.len() >= MAX_STUCK || workers.iter().any(|(_, f)| f == folder) {
            return Err(TimedOut);
        }
        *next += 1;
        workers.push((*next, folder.to_path_buf()));
        *next
    };
    let (send, receive) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("lore-worktree".to_owned())
        .spawn(move || {
            let value = work();
            // Before sending, so a caller that has the value never finds
            // this folder still marked as running.
            finished(id);
            let _ = send.send(value);
        });
    if spawned.is_err() {
        finished(id);
        return Err(TimedOut);
    }
    receive.recv_timeout(limit).map_err(|_| TimedOut)
}

fn finished(id: u64) {
    let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    running.1.retain(|(worker, _)| *worker != id);
}

/// [`RepoContext::discover`] for `dir`, within `limit`.
pub fn discover(dir: &Path, limit: Duration) -> Result<RepoContext, TimedOut> {
    let owned = dir.to_path_buf();
    within(dir, limit, move || RepoContext::discover(&owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stuck_folder_is_refused_until_its_worker_returns() {
        let folder = Path::new("/lore-test/stuck");
        let (release, wait) = mpsc::channel::<()>();
        let short = Duration::from_millis(50);
        assert_eq!(
            within(folder, short, move || wait.recv().ok()),
            Err(TimedOut)
        );
        // Refused at once, without a second thread.
        assert_eq!(within(folder, Duration::from_secs(5), || 1), Err(TimedOut));
        // Another folder is unaffected.
        assert_eq!(within(Path::new("/lore-test/other"), short, || 2), Ok(2));
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while within(folder, short, || 3) != Ok(3) {
            assert!(std::time::Instant::now() < deadline, "worker never freed");
            std::thread::sleep(short);
        }
    }
}
