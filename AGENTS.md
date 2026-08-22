# AGENTS.md — working on Lore

> Instructions for humans **and** coding agents contributing to this repository. Read this before making changes. It is intentionally short and points to canonical docs; do not duplicate their content here.

## What Lore is
Lore is a **local-first desktop app that turns the sessions your coding agents already write to disk into a searchable, git-anchored knowledge base** — running entirely on the user's machine, no account, no cloud. Tagline: *"Your coding agents forget. Lore doesn't."* Positioning: **"git memory for coding agents"** — the differentiator is provenance-aware git *depth* (session-recorded commit/patch evidence where present, ingest-time state where not, worktree/rebase-aware) + git-evidenced skills, **not** cross-agent viewing (table stakes; competitors like CCHV already do it). Go **narrower + deeper** (Claude Code + Codex), not broader. Full framing + the competitive reasoning: `docs/product/VISION.md`, `docs/product/PRD.md`, `docs/research/COMPETITIVE_LANDSCAPE.md` — all *(internal, not in this repository; see `docs/DOCS_INDEX.md`)*.

> **Status: active implementation.** The repository contains a Rust archive core, Claude Code and Codex adapters, SQLite storage/search, Git enrichment, a Tauri shell, and a React UI. It is not packaged for general installation. Treat `docs/product/ROADMAP.md` *(internal)* as the canonical implementation-status record, and never claim an acceptance gate passed without running it.

## Core principles (non-negotiable)
1. **Local-first & private by default.** Archive modules have no network capability; the separate update check is off by default and sends only documented release fields when explicitly invoked/enabled. No telemetry, accounts, or LLM calls in V0. Enforced by dependency/call-site guards and OS-level egress tests. See `docs/architecture/SECURITY.md`, `LOCAL_FIRST.md`, ADR-0005.
2. **Read, don't wrap.** We ingest agents' on-disk logs **read-only**; we never modify or wrap the agents. New agent = new **adapter**, not a fork.
3. **Git evidence keeps provenance.** Agent-recorded values, agent-recorded patches, Lore's ingest-time observation, and later re-verification are first-class and never blurred into a fictional exact session-time snapshot. `docs/architecture/GIT_INTEGRATION.md`.
4. **Fidelity before synthesis.** Store what happened faithfully; interpret (skills, summaries) later, always traceable to evidence.
5. **Tolerant parsing.** Never hard-fail on unknown/partial/newer input; degrade to `partial` with a note; isolate adapter panics. `docs/architecture/AGENT_ADAPTERS.md` §5.
6. **Simple, observable, testable, replaceable** over clever/distributed. One process, one primary SQLite database plus bounded app-owned support files, adapters behind a trait. No microservices/daemon/cloud.

## Architecture (one screen)
Tauri 2 app: **Rust core** (discovery · watcher · adapters · ingest · git · storage(SQLite+FTS5) · search · secrets · jobs) + **React/TS UI** over a generated, versioned IPC contract. Details: `docs/architecture/ARCHITECTURE.md`. Data model: `docs/architecture/DATA_MODEL.md`.

## Repository layout
- `docs/` — canonical documentation (start at `docs/DOCS_INDEX.md`).
- `crates/lore-core/` — testable Rust archive core (adapters, ingest, Git, storage, search, safety, jobs).
- `crates/lore-ipc/` — versioned Rust DTOs and generated TypeScript bindings.
- `src-tauri/` — thin Tauri command and application layer.
- `src/` — React/TypeScript UI.
- `AGENTS.md` (this) · `CLAUDE.md` · `README.md` · `RESEARCH_SUMMARY.md` *(internal)*.

## Development
Commands, layout, and conventions: `docs/development/DEVELOPMENT.md`. Testing (fixtures, guards, perf): `docs/development/TESTING.md`. Releases: `docs/development/RELEASES.md`.
> Commands now exist, but **run before you claim**. Report exactly what ran and what did not (global rule).

## Coding conventions (summary)
- Rust: `fmt` + `clippy -D warnings`; no `unwrap()/panic!` on untrusted input; errors at boundaries.
- TS: strict; heavy work in Rust via IPC, never in the webview.
- IPC DTOs are **generated** from Rust (`ts-rs`) — never hand-edit the contract.
- Adapters: implement the trait, add fixtures + a `docs/agents/<AGENT>.md`, degrade gracefully.
- Migrations: additive-first; every schema change = migration + `DATA_MODEL.md` update + test.
- Every bug fix gets a regression fixture.

## Architectural constraints (hard limits)
- **No network capability in archive modules.** The updater is the sole V0 network-capable component and is off by default. Any other off-machine flow requires a **new ADR** + `SECURITY.md` data-flow review + explicit opt-in.
- **No server / daemon / hosted DB / auth / Kubernetes / microservices.**
- **No LLM calls in V0.** Skill synthesis (V0.5+) needs a settled privacy model first.
- **Read-only** on all agent files. Never write/rename/delete a user's session logs or repos.
- Opaque/encrypted agent fields (e.g. Codex `encrypted_content`) are **never** indexed or exported.

## What NOT to do
- Don't turn Lore into an IDE, agent runtime/orchestrator, forward-memory/RAG injector, note-taking app, or SaaS. See `docs/product/NON_GOALS.md` *(internal)*.
- Don't add Claude-specific code outside an adapter.
- Don't introduce embeddings/semantic search before FTS is proven insufficient (ADR-0004).
- Don't fake screenshots or claim unbuilt features (README must stay honest).
- Don't test parsers against real user history; use anonymized fixtures.
- Don't spawn subagents/orchestration for routine work unless the user asks.

## Documentation you MUST keep in sync (self-maintaining docs)
If your change affects any of the following, update the **canonical doc in the same change** (map in `docs/DOCS_INDEX.md`). This is a review gate, not a suggestion:

| If you change… | Update… |
|---|---|
| the DB schema / entities | `docs/architecture/DATA_MODEL.md` (+ a migration + test) |
| an agent's parsing / a new adapter | `docs/agents/<AGENT>.md` and `docs/architecture/AGENT_ADAPTERS.md`; if cross-cutting, also `docs/research/AGENT_STORAGE_FORMATS.md` *(internal)* |
| the IPC/command/event contract or a CLI/API surface | `docs/architecture/ARCHITECTURE.md` §5 (+ regenerate TS types) |
| git identity/snapshot/worktree logic | `docs/architecture/GIT_INTEGRATION.md` |
| search behavior/ranking/tokenizer | `docs/architecture/SEARCH.md` |
| any security/privacy assumption or a new data flow | `docs/architecture/SECURITY.md` (+ possibly a new ADR) |
| a UX flow / screen | `docs/design/WIREFRAMES.md` / `INFORMATION_ARCHITECTURE.md` *(both internal)* |
| a significant technical decision | add/append an **ADR** in `docs/decisions/` (Context/Options/Decision/Why/Tradeoffs/Consequences/Revisit) |

Rule of thumb: **if the change would make a doc's statement false, fix the doc in the same PR.** Prefer updating the one canonical source (no duplicated truth).

## Nested AGENTS.md
Subsystem-specific instructions may live in nested `AGENTS.md` files (e.g. `src-tauri/src/adapters/AGENTS.md` once code exists) covering that subsystem's invariants. Keep them thin and local.

## Open decisions (need human input — don't unilaterally resolve)
See `RESEARCH_SUMMARY.md` *(internal)* §9: the **skill-promotion privacy model** and confirmation of the **Tauri** choice remain open — flag, don't silently decide. **Naming is DECIDED (2026-08-10): keep "Lore" everywhere** (product + skill feature), differentiating via git-evidenced skills; a trademark/SEO check is still owed before public launch.
