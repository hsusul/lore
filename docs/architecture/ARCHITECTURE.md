# Architecture

> Status: implemented foundation, evolving toward V0 acceptance. Companion: `DATA_MODEL.md`, `AGENT_ADAPTERS.md`, `GIT_INTEGRATION.md`, `SEARCH.md`, `SECURITY.md`, `LOCAL_FIRST.md`, and ADRs `0001`–`0006`. Tags: **DECISION** / **INFERENCE** / **OPINION**.

## 1. One-paragraph shape

Lore is a **single desktop process** with two halves: a **Rust core** (discovery, watching, agent adapters, normalization, Git enrichment, SQLite storage, search, background jobs) and a **web-tech UI** (React + TypeScript) rendered in the OS webview via **Tauri 2**. There is **no server, no daemon, no cloud**. State lives in one app-private directory (SQLite plus blobs, backups, logs, and disposable caches). The core exposes a typed command/event API to the UI over Tauri IPC. Archive functionality is offline-capable and account-free.

## 2. Technology decisions (evaluated, not defaulted)

See ADRs for full reasoning; summary here.

- **Desktop shell → Tauri 2** (ADR-0001). vs Electron: ~10× smaller binaries, no bundled Chromium, Rust backend is exactly the language we want for fast filesystem/Git/SQLite work. vs native Swift (Claudoscope's choice): native is fastest but **mac-locks us**, which the brief says to avoid. Tauri keeps macOS-first *and* a real Windows/Linux path. Cost: system-webview rendering quirks (acceptable for our UI).
- **UI → React + TypeScript + Vite.** DECISION. Mature, huge ecosystem for the dense, list-heavy, keyboard-driven UI we want (virtualization, command palette, diff viewers). Alternatives (Svelte/Solid) are fine but React maximizes hireability/contributor familiarity for an OSS project. Discipline required: virtualize big lists, keep heavy work in Rust.
- **Core language → Rust** (ADR-0001/0003). The workload is filesystem-, Git-, and DB-heavy — Rust gives us performance, safety, `gix`, `rusqlite`, `notify`, and a clean adapter trait system. TS/Node core was considered and rejected (worse for CPU-bound parsing at 1M+ messages; we'd still want Rust for indexing).
- **Storage → SQLite + FTS5** (ADR-0002, ADR-0004). Embedded, zero-config, battle-tested, single-file, great for 10k sessions/1M+ rows. FTS5 gives V0 full-text search with no extra infra. DuckDB considered for analytics (V0.5 cost rollups) but not needed for V0; can attach later. Vectors (`sqlite-vec`) deferred to V1.
- **Git → `gix` (gitoxide) primary, system `git` fallback** (`GIT_INTEGRATION.md`). Pure-Rust, fast, no libgit2 C dependency; shell-out covers gaps/edge cases.
- **File watching → `notify`** (wraps FSEvents on macOS). Matches the poll-free approach Claudoscope validated.

## 3. Layered module map (Rust core)

```
┌─────────────────────────────────────────────────────────────────┐
│  UI  (React/TS in system webview)                                │
│   Repositories · Sessions · Session Detail · Search · Settings   │
└───────────────▲───────────────────────────────┬─────────────────┘
   commands (req/resp)                 events (push: scan progress,
   e.g. search(), get_session()          new session, index done)
                │                                 │
┌───────────────┴─────────────── Tauri IPC ───────┴─────────────────┐
│                          app / command layer                       │
│  validates input · maps to core services · streams events          │
├────────────────────────────────────────────────────────────────────┤
│  services                                                          │
│   ┌───────────┐ ┌───────────┐ ┌───────────┐ ┌───────────┐          │
│   │ Discovery │ │  Watcher  │ │  Ingest   │ │  Search   │          │
│   │ (scan)    │ │ (notify)  │ │ pipeline  │ │ (FTS5)    │          │
│   └────┬──────┘ └────┬──────┘ └────┬──────┘ └────┬──────┘          │
│        │             │             │             │                 │
│        └─────────────┴──────►  Job queue  ◄──────┘                 │
│                    (bounded worker pool, backpressure)             │
├────────────────────────────────────────────────────────────────────┤
│  domain                                                            │
│   AgentAdapter registry ── normalize ──► unified model             │
│   GitService (identity, snapshot, enrich)                          │
│   SecretScanner (index-time)                                       │
├────────────────────────────────────────────────────────────────────┤
│  storage                                                           │
│   SQLite (rusqlite) · FTS5 · migrations · blob/cache dir           │
└────────────────────────────────────────────────────────────────────┘
```

**Module responsibilities (interfaces, not implementations):**

- **Discovery** — enumerate candidate session files under each adapter's documented defaults plus persisted user-selected roots; produce `SessionRef { adapter, path, mtime, size }`. Idempotent; cheap; safe to re-run.
- **Watcher** — `notify`-based; debounced; emits `FileChanged(path)` → schedules an ingest job. Adding/removing a custom root replaces the worker's discovery config and watcher, then queues an incremental scan without restarting the app. One truncated/partial line must not fail the batch (Risk R5).
- **Ingest pipeline** — identify source/generation → parse/normalize in bounded batches → Git enrich → stream secret scan and prepare redacted projections/blob temp files → transactionally persist canonical rows/findings/SearchDocument/FTS/checkpoint. Expensive scanning occurs before the write transaction; content-addressed temp blobs are atomically finalized and orphan-cleaned after crashes.
- **AgentAdapter registry** — trait objects (see `AGENT_ADAPTERS.md`); adapters are the *only* place agent-specific code lives.
- **GitService** — ambiguity-aware repository identity evidence, segment-level recorded/captured/reverified observations, and hardened read-only fallback (see `GIT_INTEGRATION.md`).
- **SecretScanner** — complete streaming scan of every persisted cleartext surface; controls SearchDocument/export/cache eligibility (see `SECRET_SCANNING.md`).
- **Search** — builds/queries FTS5 over redacted SearchDocument projections; applies structured filters; returns marker-highlighted snippets with source navigation.
- **Job queue** — bounded worker pool with backpressure; priorities (user-triggered > background reindex); cancellable; survives crashes via a durable `job`/`ingest_state` table.
- **Storage** — SQLite with WAL, versioned migrations, explicit Blob references, bounded local backups, and disposable caches. See DATA_MODEL.

## 4. Concurrency & process model

- **Single process.** Rust core runs on a Tokio (or std-threadpool) runtime inside the Tauri app; UI in the webview. No separate daemon.
- **Background work is bounded and cancellable.** First scan and reindex run as jobs with progress events; the UI stays responsive.
- **DB access** via a small connection pool; **WAL mode** so reads never block the ingest writer. Long scans batch writes in transactions.
- **Backpressure:** the ingest queue is bounded; discovery yields refs lazily so a 100k-session first scan can't OOM.

## 5. IPC contract (UI ↔ core)

Two channels (DECISION):
- **Commands** (request/response), implemented in `src-tauri/src/lib.rs`: `core_version()`, `list_detected_agents()`, `add_agent_root(agent_id, path)`, `remove_agent_root(agent_id, path)`, `list_sessions(limit)`, `list_sessions_page(limit, cursor)`, `list_repositories()`, `list_repository_sessions(id, limit)`, `list_repository_sessions_page(id, limit, cursor)`, `get_session(id)`, `get_file_patch(id)`, `get_git_snapshot(id)`, `session_secret_count(id)`, `export_session_markdown(id, include_secrets)`, `forget_session(id)`, `forget_everything()`, `search(query, limit)`, `search_page(query, limit, cursor, sort)`, `list_folders()`, `create_folder(name)`, `rename_folder(id, name)`, `delete_folder(id)`, `set_session_folder(session_id, folder_id)`, `list_folder_sessions_page(id, limit, cursor)`, `get_setting(key)`, `set_setting(key, value_json)`, `get_backup_schedule()`, `set_backup_schedule(interval, keep)`, `backup_now()`, `rescan()`. Inputs are validated at the boundary (including unified `is_invalid_text_token` length bounds, limit clamping, control-character/zero-width character rejection, and setting payload size limits). `list_detected_agents()` live-probes every registered adapter even before the first ingest and returns effective/custom roots plus archived-session counts. Root additions accept only registered adapters and existing absolute directories below the filesystem root; documented defaults cannot be removed. The browse-page commands return `SessionPage { sessions, next_cursor }` over the stable `(started_at DESC, id DESC)` key; the cursor is opaque, missing timestamps sort last, and malformed cursors safely restart at page one. Browse/search rows intentionally use the compact `SessionSummary`; only `get_session()` returns the nullable, bounded, content-free `parse_note` on `SessionDetail` for degraded-session explanation.
- **Events** (core → UI push): currently only `scan_progress` (content-free, per-scan counts). A discovery pass resets its counters and emits `done = true` only after its scheduled work drains, so repeated scans do not inflate historical skip/failure totals. Planned: `session_ingested`, `index_updated`, `job_failed`, `secret_flagged` for live ingestion without polling.

All DTOs are versioned TS types generated from Rust (e.g. `ts-rs`) so the contract can't silently drift — a CLI/API-contract change **must** update generated types + `DOCS_INDEX.md` (see AGENTS.md self-maintenance rule).

## 6. Data flow: first run

1. On app startup a **background worker** (its own SQLite connection on a dedicated thread) loads documented plus persisted custom roots, recovers jobs left `running` by a prior crash, then runs the initial incremental scan automatically — enumerating adapters → Discovery yields `SessionRef`s (lazy). A manual `rescan()` queues the same scan and returns immediately. A native directory dialog lets the user add a root; the backend persists the canonical path, replaces the worker config/watcher, and scans it asynchronously. None of these paths parse source histories while holding the UI database connection.
2. Ingest jobs identify SourceArtifacts, stream normalized rows and GitObservations through complete scanning/redaction, then transactionally commit canonical rows + findings + SearchDocument/FTS + checkpoint. Draining is bounded (batched claim→ingest→finish) so the worker never holds the DB lock across parsing/Git and the UI connection stays responsive.
3. `scan_progress` events (content-free counts) drive a progress UI and a debounced list refresh; repos/sessions populate incrementally (time-to-wow < 60 s target). First-scan jobs prioritize recently modified source files so current work becomes usable before older large archives finish.
4. The debounced watcher keeps running on the worker thread; new/changed/appended sessions coalesce into durable jobs and re-ingest automatically without a manual rescan. On exit the worker finishes its current bounded step and joins; interrupted work stays durable and is recovered on the next launch.

## 7. Failure & resilience

- **Tolerant parsing:** unknown event `type`, unknown content block, truncated final line, unknown agent version → skip/degrade, never crash (Risks R2/R5). Each session records a `parse_status` (`ok | partial | failed`) plus a bounded, content-free diagnostic; the opened session explains what Lore recovered while browse/search payloads remain compact.
- **Crash-safe ingest:** restart resumes only from a checkpoint committed with its normalized/search rows and only after source generation/size/prefix validation; rewrites rebuild the affected generation.
- **Corruption:** integrity checks close and preserve the damaged DB, then offer restore from local backup, salvage, or best-effort re-scan of source logs that still exist. Lore never assumes agents retained them.
- **Isolation:** a failing adapter job does not commit partial state or stop other adapters. Panic catching is a last-resort guard, not a sandbox; input/resource/time bounds provide containment.

## 8. Observability (local only)

- Structured logs to an app-private log file (rotating), redaction-aware (never log raw secrets). A "Logs" affordance in Settings is planned.
- **No remote error reporting by default.** Any future crash-reporting is opt-in and scrubbed (see SECURITY).

## 9. Updates & distribution

- A capability-separated signed Tauri updater; scheduled checks are **off by default**, and manual check is an explicit network action. Signed + notarized `.dmg`; Homebrew cask later. Details in `docs/development/RELEASES.md` and `SECURITY.md` §7.

## 10. Why this is the right altitude (OPINION)

It's deliberately *small*: one process, one primary SQLite database plus bounded app-owned support files, adapters behind a trait, Git as a service, search as FTS5. Every piece is independently testable and replaceable without microservices, a daemon, a hosted index, or premature embeddings.
