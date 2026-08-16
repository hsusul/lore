# Roadmap & Implementation Plan

> Milestones that each produce working, independently-mergeable software. Companion: `PRD.md`, `docs/architecture/*`, `docs/development/TESTING.md`. Each milestone: **Goal · Tasks · Modules · Tests · Done-when · Risks.** Tags: **DECISION** / **OPINION**.

> **Last reconciled with `main`: 2026-08-16.** Status here reflects code and committed tests, not intent. “Built” means the milestone's main path exists; unchecked acceptance work still blocks calling the milestone complete.

## Current state

| Milestone | State | What is true now | What remains |
|---|---|---|---|
| M0 Foundation | Built | Cargo workspace, Tauri shell, React/Vite UI, migrations, generated IPC DTOs, CSP/capabilities, CI, Apache-2.0 license | Finish updater isolation/contract testing and release-grade egress harness |
| M1 Claude ingestion | Built | Read-only discovery, tolerant adapter, normalized persistence, checkpoints, lifecycle tests | Expand versioned and large-file fixture coverage as formats evolve |
| M2 Codex ingestion | Built | Rollout parsing, call pairing, patches, Git metadata, compaction, opaque exclusion, token totals | Continue version-fixture coverage; keep capability flags honest |
| M3 Live pipeline | Built | Registry, discovery, debounced watcher, durable coalescing queue, scan/ingest/enrich pipeline, progress events, **background worker wired into app startup/shutdown** (own connection + watcher, initial incremental scan, bounded batched drain, restart recovery, observable backpressure, clean shutdown); **deterministic synthetic-profile generator + 10k-session ingest proof** (~42 s release, bounded, no OOM); debug-only `LORE_DEV_*` root override for `cargo tauri dev` on seeded fixtures; **headless desktop E2E** (launch→scan→live FSEvents update→browse→search→export→forget) which proved and fixed a symlinked-root canonicalization bug in watcher→owner resolution | Optional: broaden E2E to a real webview driver once a Linux/Windows CI lane exists (macOS WKWebView has no WebDriver) |
| M4 Git enrichment | Built, acceptance pending | Worktree/repository identity, credential-safe remotes, capture, provenance labels, hardened fallback, re-verification, UI rail | Complete lifecycle/hostile-config acceptance matrix and validate grouping on synthetic multi-worktree profiles |
| M5 Browse UI | Built, polish pending | Three-pane browser, repository/session queries, **user-defined folder organization** (inline create/rename/delete, drag-and-drop thread filing, keyset-paged folder views), **windowed + stable keyset-paged session list** (bounded first page, explicit older-page continuation, no silent 500-row ceiling), ordered timeline, patches, a keyboard-first command palette with immediate ordered-fuzzy matching, debounced archive recall, latest-query ownership, Home/End navigation, and focus restoration, a **truthful first-run empty-archive panel** (distinct initial loading/failure states, local/read-only boundary, live supported-root detection before ingest, native custom-folder selection, working scan/settings actions), **persisted default/custom root management with hot worker/watch reconfiguration**, and a **truthful degraded-session panel** (partial/failed explanation, recovered counts and Git availability, bounded content-free parser diagnostic, original-log reassurance) | Deeper skipped-count/report diagnostics, accessibility and desktop E2E coverage |
| M6 Search | Baseline built | Secret scanning, redacted projections, FTS5 query API, filters, snippets, search UI with a continuous keyboard query→results→query loop, **stable keyset pagination** (`search_page` + opaque cursor over `(bm25, started_at DESC, id)`, with `SortOrder::Relevance`, `SortOrder::Newest`, and `SortOrder::Oldest` modes, exposed via the `search_page` IPC command), UI "load more" paging, and command-palette archive recall over the same redacted index | Ranking-boost tuning (field/exact-match); prove secret coverage and the 1M-message latency target with committed reports |
| M7 Safety and polish | In progress | Secret badges, redaction-aware export (with title newline and file path backtick formatting sanitization), forget-session, forget-everything, privacy/settings surface, **user-visible backup cadence/settings and manual backup triggers** (`BackupSettings` UI + IPC schedule/restore wiring), **Lore-owned local backup create + restore + recovery wiring** (SQLite online backup into `backups/`, retention-pruned, `integrity_check`-verified, private perms; `restore_backup` + `list_backups` re-verify the destination without source logs; `recover_archive` quarantines a corrupt archive under `quarantine/` — never discarded automatically — and restores the newest backup, or reports the preserved quarantine path), **scanner-failure quarantine** (fallible `secrets::scan` — a scanner defect on untrusted input is captured, never a panic; the field is held out of search/export, no findings recorded, and its recorded-patch blob is marked `failed_quarantined`), **boundary input validation** (length bounds, limit clamping, canonical `is_zero_width` detection, and control-character sanitization across all session, repository, search, backup schedule, setting, and folder IPC commands) | Complete deletion sweep, offline/egress and accessibility acceptance |
| M8 Release | Not started | Release design is documented | Signed/notarized macOS bundle, updater, clean-machine QA, release automation and public install docs |

## Next build sequence

1. **Make the desktop pipeline continuous.** ✅ Done — the watcher starts with the app, debounced changes become durable coalescing jobs, a bounded background worker (own connection/thread) drains them, content-free progress is emitted, and the worker shuts down cleanly recovering interrupted jobs. Sources stay read-only.
2. **Close the safety loop.** ✅ Lore-owned local backup **creation, restore, and recovery wiring** landed (SQLite online backup, retention-pruned, integrity-verified; restore re-verifies the destination without source logs; `recover_archive` preserves a corrupt archive under `quarantine/` — never discarded automatically — and restores the newest backup, or reports the preserved quarantine path). ✅ **Scanner-failure quarantine** landed: `secrets::scan` is fallible — a scanner defect on untrusted input is captured (never a panic), the field is held out of search/export with no findings recorded, and its recorded-patch blob is marked `failed_quarantined` (SECRET_SCANNING.md §6). ✅ **User-visible backup cadence/settings** landed in Settings with interval/retention controls and manual "Back up now". ✅ **IPC boundary input hardening** landed across session, repository, search, backup schedule, setting, and folder commands (including control-character and zero-width validation). ✅ **Redaction-aware markdown export hardening** landed (masking secrets by default and neutralizing title newline / code block breakout vectors). Still owed: the final deletion-sweep audit of every Lore-owned sidecar/blob/cache/log while clearly reporting original agent logs and external exports that remain.
3. **Prove the current product at scale.** ✅ Deterministic synthetic profile generator (`lore_core::synthetic`) + 10k-session ingest test landed (10k streams to completion in ~42 s release, queue bounded, no OOM); debug-only `LORE_DEV_*` root override lets `cargo tauri dev` run on a seeded profile; ✅ headless desktop E2E over a synthetic profile (launch→scan→live update→browse→search→export→forget). Still owed: the ~1M-message search report (M6) and OS-level deny-egress verification.
4. **Finish the interface, then package it.** ✅ The session browser is windowed and keyset-paged. ✅ A successfully loaded empty archive gets privacy-first onboarding without a false-empty loading flash, including live adapter detection and native custom-folder selection. ✅ Custom roots persist, remain additive to defaults, and hot-reconfigure scanning/watch coverage. ✅ Opened partial/failed sessions explain the recovered data, show a bounded content-free parser diagnostic, and keep the recovered timeline available. Still owed: deeper skipped-count/report diagnostics, complete accessibility states, fresh-profile QA, and only then the signed/notarized macOS release path.

Do not add more agents, embeddings, an MCP server, or skill synthesis before these gates are closed. The wedge still needs to be proven through Git-aware recall quality, not feature count.

## Guiding sequencing principle
Ship the **git-anchored searchable archive** (the wedge, fully local, no LLM) end-to-end before any synthesis. Each milestone leaves `main` releasable. Prefer vertical slices (a thin path through all layers) over horizontal (a whole layer at once) so we always have a runnable app.

---

## M0 — Repository & app foundation
- **Goal:** an empty-but-runnable Tauri app + Rust core skeleton + CI + docs wired.
- **Tasks:** Tauri 2 scaffold (React+TS+Vite); Rust core crate layout; schema/migrations + `ts-rs`; updater as a separate default-off capability; CSP; dependency/call-site guard; OS-level egress harness; content-free logging; CI and license audit.
- **Modules:** all (skeleton).
- **Tests:** IPC round-trip; migration up/down; non-updater network dependency/raw-socket call fails CI; full default workflow attempts no egress; explicit mocked updater sends only documented fields.
- **Done-when:** app launches to an empty shell; `cargo test`/`npm test` pass in CI; `docs/DOCS_INDEX.md` resolves.
- **Risks:** Tauri/webview setup friction; keep the shell minimal.

## M1 — Claude Code ingestion (vertical slice)
- **Goal:** discover, parse, and persist Claude Code sessions into the normalized schema.
- **Tasks:** `ClaudeCodeAdapter` with configurable root discovery; normalization → SourceArtifact/AgentSession/SessionSegment/Message/ordered MessagePart/ToolCall/FileEvent; transactional canonical upserts + validated checkpoint (SearchDocument/FTS comes in M6); tolerant parsing.
- **Modules:** adapters, ingest, storage.
- **Tests:** fixtures from `docs/agents/CLAUDE_CODE.md` §8; mixed blocks/string content/cwd changes round-trip; append/truncate/rewrite/archive move/restart idempotency; large fixture streams with bounded memory.
- **Done-when:** pointing at a fixtures dir yields correct sessions/messages/tool calls in SQLite.
- **Risks:** schema drift → mitigated by version-keyed fixtures + additive parsing.

## M2 — Codex ingestion
- **Goal:** second adapter proves the abstraction; adds optional recorded Git and path-keyed patch evidence.
- **Tasks:** `CodexAdapter` (rollout JSONL, `call_id` pairing, `patch_apply_end.changes[path]` payload preservation/count derivation, sparse `session_meta.git`, changing turn context, opaque exclusion, compaction).
- **Modules:** adapters, ingest.
- **Tests:** fixtures from `docs/agents/CODEX.md` §8; encrypted-region exclusion; compaction marker; non-openai provider guard.
- **Done-when:** Claude + Codex sessions coexist in one normalized store via the same pipeline.
- **Risks:** Codex format churn → key off `cli_version`.

## M3 — Discovery, watching & background jobs
- **Goal:** zero-config first scan + live updates, responsive under load.
- **Tasks:** discovery over real roots; `notify`/FSEvents watcher (debounced) → ingest jobs; bounded worker pool + durable `job` queue + backpressure; `scan_progress`/`session_ingested` events.
- **Modules:** discovery, watcher, jobs, app/IPC.
- **Tests:** 10k fixture scan stays responsive; new-file appears; archive move dedupes; crash resumes only from committed checkpoint; prefix mismatch/truncation rebuilds the source generation.
- **Done-when:** launch → background scan populates incrementally; new sessions show up live.
- **Risks:** watcher storms / partial writes → debounce + last-complete-line parsing.

## M4 — Git enrichment
- **Goal:** every segment carries the strongest available, honestly labeled repository/Git evidence and survives repository changes.
- **Tasks:** `GitService`; multi-signal RepositoryIdentityEvidence; worktree/common-dir resolution; separate agent-recorded/agent-patch/Lore-captured/reverified GitObservations; identity confidence and merge/split; hardened Git fallback.
- **Modules:** git, ingest (enrich stage), storage.
- **Tests:** N worktrees→1 repo; fork sharing root stays separate; unambiguous move/clone relinks; cwd changes segment; retrospective Claude never claims exact historical diff; rebase/delete flags; hostile Git config cannot execute helpers/network.
- **Done-when:** Repositories view groups sessions correctly across worktrees; git rail shows as-recorded vs captured.
- **Risks:** git edge cases → shell-out fallback + fixtures for each lifecycle event.

## M5 — Repository & session UI
- **Goal:** the app is usable: browse repos, read sessions in context.
- **Tasks:** three-pane shell; Repositories view with identity confidence; user-defined folders and drag-and-drop thread filing; virtualized Sessions list; ordered-part timeline; recorded/captured/reverified Git labels and observation times; inline recorded patches; command palette; onboarding + error states; theming.
- **Modules:** UI + IPC commands (`list_repositories/list_sessions/get_session/get_git_snapshot/list_folders/create_folder/rename_folder/delete_folder/set_session_folder`).
- **Tests:** UI/component tests; virtualization at 10k rows; keyboard nav; folder CRUD and pagination; partial-parse + missing-repo states render (Wireframes §10).
- **Done-when:** a user can go launch→scan→browse repo→read a session with git context, keyboard-only.
- **Risks:** webview perf on huge lists → virtualize; keep heavy work in Rust.

## M6 — Full-text search
- **Goal:** the killer recall path.
- **Tasks:** complete streaming secret-scanner/quarantine prerequisite; redacted SearchDocument projections + external-content `search_fts`; tokenizer configuration; BM25/boosts; provenance-aware filters; safe query parser; `snippet()` marker highlighting; atomic rebuild.
- **Modules:** search, storage, UI.
- **Tests:** identifier/error/path/provenance recall; stable keyset pagination across relevance/newest/oldest sort orders; warm/cold/adversarial <200 ms target report at 1M messages; chunk-boundary/middle-of-blob secrets absent from derived surfaces.
- **Done-when:** "find that thing" works across all sessions with filters, fast.
- **Risks:** tokenization gaps → fixture-driven tuning.

## M7 — Safety, settings & polish (V0 release candidate)
- **Goal:** trustworthy, shippable V0.
- **Tasks:** expand/tune secret rules + UI badges/reveal; redaction-aware export; Settings (privacy/data/backup/recovery/forget/rescan/custom source roots); IPC boundary validation (length bounds, control/zero-width character rejection, payload size caps); truthful threat-boundary copy; Agents screen; deletion sweep; egress boundary verified; accessibility pass.
- **Modules:** secrets, app, UI, storage.
- **Tests:** planted large-field secrets redacted in all derived surfaces; scan failure quarantines; “Forget everything” removes all Lore-owned DB sidecars/blobs/backups/caches/logs and reports non-owned copies; backup recovery without sources; IPC boundary sanitization tests; offline/a11y tests.
- **Done-when:** V0 acceptance criteria (`PRD.md` §9) met; the default archive workflow attempts zero outbound connections and the explicit updater is the only permitted path.
- **Risks:** secret false-positives/negatives → tunable rules + tests.

## M8 — Packaging & release
- **Goal:** downloadable, signed macOS app + update channel.
- **Tasks:** signed + notarized `.dmg`; Tauri updater (opt-in) over GitHub Releases; Homebrew cask; crash-safe first-run; README/site with honest "under development" status.
- **Modules:** build/release.
- **Tests:** fresh-machine install; update flow; notarization passes; first-run scan on a real profile.
- **Done-when:** a user downloads Lore and reaches time-to-wow<60s. See `docs/development/RELEASES.md`.
- **Risks:** Apple signing/notarization friction → budget time; document the pipeline.

---

## Post-V0 (mapped to PRD V0.5 / V1)
- **V0.5:** Gemini + OpenCode adapters · SpecStory Markdown fallback adapter · saved searches/cross-repo search · cost/activity analytics · **git-evidenced skill promotion** (opt-in privacy model — design in `docs/architecture/SKILL_EXTRACTION.md`).
- **V1:** Cursor adapter (beta) · local hybrid semantic search (`sqlite-vec`) · read-only local MCP "query the archive" · worktree/parallel-agent intelligence · Windows/Linux builds.

---

## Performance plan (scale targets & tactics)

| Scale | Sessions | Messages | Expectation | Key tactics |
|---|---|---|---|---|
| Small | 10 | ~1k | instant everything | trivial |
| Typical | 1,000 | ~100k | scan in seconds; instant search | streaming ingest; indexed filters |
| **Target** | **10,000** | **~1M** | first scan minutes (background, incremental); **<200 ms typical search** on reference laptop; smooth scroll | bounded transactional ingest; SearchDocument/FTS; virtualization; keyset pagination; WAL |
| Heavy | 100,000 | ~10M+ | usable; scan is background; search stays fast | batch txns; content-hash skip; per-repo lazy loading; measure before sharding |
| Extreme | — | millions/session | never OOM | streaming parse; offload big text to `blobs/`; never load whole file |

**Likely bottlenecks (OPINION):** parsing + complete secret scans; SearchDocument/FTS build and rebuild; virtualized rendering; repeated Git capture; huge blobs. Use bounded concurrency, coalesced Git observations, explicit blobs, atomic index rebuilds, and measured warm/cold/adversarial benchmarks before considering sharding/embeddings.
