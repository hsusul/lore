//! Desktop end-to-end acceptance (pre-release tier — `#[ignore]`d).
//!
//! Drives the **real product flow** headlessly: the same background worker the
//! Tauri app spawns at startup, with a real recursive [`SessionWatcher`], over a
//! deterministic synthetic profile — then exercises every `lore_core` entry
//! point the `#[tauri::command]` layer wraps (agents, repositories, sessions,
//! session detail, git snapshot, secret count, search, export, forget). It also
//! validates live ingestion under **real FSEvents**: a session that appears
//! after startup is ingested with no manual rescan.
//!
//! This is `#[ignore]`d because it depends on real OS filesystem-watch delivery
//! (timing, not deterministic) and belongs to the pre-release E2E tier in
//! `docs/development/TESTING.md`, not the every-commit gate. Run it with:
//!
//! ```text
//! cargo test -p lore-core --test e2e -- --ignored --nocapture
//! ```
//!
//! No real `~/.claude` / `~/.codex` history is touched — the profile is fully
//! synthetic (`lore_core::synthetic`).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use lore_core::adapters::AdapterRegistry;
use lore_core::discovery::watch_roots;
use lore_core::pipeline::{ProgressEvent, ProgressSink};
use lore_core::storage::blob::BlobStore;
use lore_core::synthetic::{generate, ProfileSpec};
use lore_core::watcher::SessionWatcher;
use lore_core::worker::{open_worker, spawn, WorkerConfig};
use rusqlite::Connection;

/// `Send` sink that forwards content-free progress over a channel, mirroring how
/// the app relays `scan_progress` to the webview.
struct ChannelSink(mpsc::Sender<ProgressEvent>);
impl ProgressSink for ChannelSink {
    fn emit(&self, event: ProgressEvent) {
        let _ = self.0.send(event);
    }
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Poll `predicate` up to `timeout`, returning whether it became true. Used only
/// to await real, asynchronous FSEvents delivery — an E2E waiting on the OS, not
/// a sleep papering over a logic race.
fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    predicate()
}

const WAIT: Duration = Duration::from_secs(20);

#[test]
#[ignore = "pre-release E2E: depends on real FSEvents delivery; run with --ignored"]
fn desktop_flow_launch_scan_live_update_browse_search_export_forget() {
    // --- Arrange: a synthetic agent home + the app's real storage topology. ---
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let db_path = data.path().join("lore.db");

    let spec = ProfileSpec {
        claude_sessions: 12,
        codex_sessions: 12,
        max_extra_turns: 3,
        seed: 99,
    };
    let profile = generate(home.path(), &spec).unwrap();
    let total = (profile.claude_files + profile.codex_files) as i64;
    let config = profile.discovery_config();

    // The UI half: a read connection to the same DB file (as `AppState.db`).
    let ui = lore_core::storage::open(&db_path).unwrap();
    let ui_blobs = BlobStore::open(blob_dir.path()).unwrap();

    // The worker half: its own connection + a real recursive watcher, spawned
    // exactly as the Tauri `setup` hook does.
    let cfg = WorkerConfig {
        // Poll the watcher promptly so live updates land within the E2E window.
        idle_poll: Duration::from_millis(50),
        ..WorkerConfig::default()
    };
    let worker = open_worker(
        &db_path,
        AdapterRegistry::v0(),
        BlobStore::open(blob_dir.path()).unwrap(),
        config.clone(),
        cfg,
    )
    .unwrap();
    let watcher = SessionWatcher::new(
        &watch_roots(&AdapterRegistry::v0(), &config),
        Duration::from_millis(50),
    )
    .unwrap();
    let (tx, progress) = mpsc::channel();
    let handle = spawn(worker, Some(watcher), ChannelSink(tx));

    // --- Act 1: startup runs the initial incremental scan automatically. ---
    assert!(
        wait_until(WAIT, || count(&ui, "SELECT count(*) FROM agent_session")
            == total),
        "initial scan did not ingest all {total} sessions"
    );
    // Content-free progress reached the "webview".
    let events: Vec<ProgressEvent> = progress.try_iter().collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProgressEvent::Ingested { .. })),
        "no content-free Ingested progress events were emitted"
    );

    // --- Act 2: a NEW session appears after startup → ingested live, no rescan.
    // Written into a day directory that already exists (and is already watched),
    // mirroring Codex appending a new rollout to today's folder.
    let new_file = profile
        .codex_root
        .join("2026/08/03/rollout-e2e0000000000.jsonl");
    std::fs::create_dir_all(new_file.parent().unwrap()).unwrap();
    std::fs::write(&new_file, live_codex_session()).unwrap();
    assert!(
        wait_until(WAIT, || {
            count(&ui, "SELECT count(*) FROM agent_session") == total + 1
        }),
        "a session created after startup was not ingested by the live watcher"
    );

    // Quiesce and stop the worker cleanly before user-driven read/write actions.
    handle.shutdown();
    let sessions_now = total + 1;

    // --- Act 3: the read surface every Tauri command wraps. ---
    // list_detected_agents
    let agents = lore_core::query::list_agents(&ui).unwrap();
    assert_eq!(agents.len(), 2, "both native adapters are present");
    assert!(agents.iter().all(|a| a.installed && a.session_count > 0));

    // list_repositories (git enrich over synthetic cwds must not error).
    let _repos = lore_core::query::list_repositories(&ui).unwrap();

    // list_sessions
    let sessions = lore_core::query::list_sessions(&ui, 10_000).unwrap();
    assert_eq!(sessions.len() as i64, sessions_now);
    let started: Vec<Option<i64>> = sessions.iter().map(|s| s.started_at).collect();
    assert!(
        started.windows(2).all(|w| w[0] >= w[1]),
        "list_sessions must be newest-first"
    );

    // get_session detail (segments, ordered messages/parts, file events).
    let target = sessions
        .iter()
        .find(|s| s.message_count > 0)
        .expect("a session with messages");
    let detail = lore_core::query::get_session(&ui, &target.id)
        .unwrap()
        .expect("session detail resolves");
    assert_eq!(detail.summary.id, target.id);
    assert!(!detail.messages.is_empty());
    assert!(detail.messages.iter().all(|m| !m.parts.is_empty()));

    // get_git_snapshot + session_secret_count (must not error).
    let _git = lore_core::query::get_git_snapshot(&ui, &target.id).unwrap();
    assert_eq!(lore_core::query::secret_count(&ui, &target.id).unwrap(), 0);

    // search over the redacted projections built during ingest.
    let hits = lore_core::search::search(&ui, "backoff", 50).unwrap();
    assert!(!hits.is_empty(), "expected search hits for a known term");
    assert!(hits.iter().all(|h| !h.snippet.is_empty()));

    // export_session_markdown (masked by default).
    let md = lore_core::export::export_session_markdown(&ui, &target.id, false)
        .unwrap()
        .expect("markdown export");
    assert!(
        md.contains("# ") || md.contains('#'),
        "export has a heading"
    );

    // --- Act 4: forget one session removes it; peers are untouched. ---
    let report = lore_core::forget::forget_session(&ui, &ui_blobs, &target.id).unwrap();
    let _ = report.blobs_removed;
    assert!(
        lore_core::query::get_session(&ui, &target.id)
            .unwrap()
            .is_none(),
        "the forgotten session is gone"
    );
    assert_eq!(
        count(&ui, "SELECT count(*) FROM agent_session"),
        sessions_now - 1,
        "exactly one session was forgotten; peers remain"
    );

    // core_version is reported (trivial command).
    assert!(!lore_core::version().is_empty());
}

/// A minimal, distinct Codex rollout for the live-update step (unique id so it
/// ingests as a new session).
fn live_codex_session() -> String {
    [
        r#"{"type":"session_meta","timestamp":"2026-08-15T09:00:00.000Z","payload":{"id":"019eE2E0-0000-7000-8000-00000000e2e0","cwd":"/proj","cli_version":"0.133.0","source":"cli","model_provider":"openai","git":{"branch":"main","commit_hash":"e2e123","repository_url":"github.com/x/proj"}}}"#,
        r#"{"type":"turn_context","timestamp":"2026-08-15T09:00:00.500Z","payload":{"cwd":"/proj","model":"gpt-x","effort":"medium","turn_id":"t1"}}"#,
        r#"{"type":"response_item","timestamp":"2026-08-15T09:00:01.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"live update session"}]}}"#,
        r#"{"type":"response_item","timestamp":"2026-08-15T09:00:02.000Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ingested live via the watcher"}]}}"#,
        r#"{"type":"event_msg","timestamp":"2026-08-15T09:00:03.000Z","payload":{"type":"task_complete","last_agent_message":"done","duration_ms":3000,"turn_id":"t1"}}"#,
    ]
    .join("\n")
        + "\n"
}
