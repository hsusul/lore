# Development Guide

> How to build and work on Lore. The application scaffold and core product paths exist; packaging and several release acceptance gates remain. Companion: `ARCHITECTURE.md`, `TESTING.md`, `RELEASES.md`.

## 1. Prerequisites
- **Rust** (stable, current) + `cargo`.
- **Node** (LTS) + npm.
- **Tauri 2** prerequisites for macOS (Xcode command-line tools, WebKit).
- **git** on PATH (used as the enrichment fallback).

## 2. Repo layout
```
Lore/
  crates/lore-core/     # archive core: adapters, ingest, Git, storage, search, safety
    src/
    migrations/         # SQL migrations (versioned)
    fixtures/           # anonymized agent fixtures
    tests/              # integration and acceptance-oriented tests
  crates/lore-ipc/      # versioned IPC DTOs + generated TypeScript bindings
  src-tauri/            # thin Tauri command/application layer
  src/                  # React + TS UI (Vite)
    components/          # React components
    ipc.ts               # typed IPC client + generated bindings (crates/lore-ipc)
  docs/                 # this documentation tree
  AGENTS.md  CLAUDE.md  README.md  RESEARCH_SUMMARY.md
```

## 3. Common commands
```bash
# install
npm install

# run the app (Tauri dev: Rust core + Vite UI)
cargo tauri dev

# rust
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check

# ui
npm run check        # typecheck + lint + test in one pass
npm test             # vitest
npm run lint
npm run typecheck
npm run build

# regenerate IPC TS types from Rust (ts-rs)
cargo test -p lore-ipc

# build a release bundle (signed/notarized config in RELEASES.md)
cargo tauri build
```
> The Tauri commands require `tauri-cli` to be installed. **Do not** claim a command works until you have run it (global rule: report only what actually ran).

## 4. Conventions
- **Rust:** `rustfmt` + `clippy` clean (deny warnings in CI). Errors via `thiserror`/`anyhow` at boundaries; **no `unwrap()`/`panic!` on untrusted input** (parsers must degrade). Public core APIs documented.
- **TypeScript:** strict mode; ESLint + Prettier; UI state minimal; heavy work in Rust via IPC, never in the webview.
- **IPC:** all DTOs generated from Rust (`ts-rs`) — never hand-write the contract. Changing it updates generated types **and** `ARCHITECTURE.md` §5 + `DOCS_INDEX.md`.
- **Adapters:** follow `AGENT_ADAPTERS.md`; add fixtures + a `docs/agents/<AGENT>.md`; never hard-fail on unknown input; isolate panics.
- **Migrations:** additive-first; every schema change = a migration + `DATA_MODEL.md` update + a migration test.
- **Commits:** small, focused, conventional-ish messages; each milestone stays independently mergeable (ROADMAP).
- **No network capability in archive modules.** The updater is a separate, default-off capability; dependency/call-site guards plus OS-level egress tests enforce the boundary.

## 5. Working with real data locally (careful)
- For manual QA you may point adapters at real `~/.claude`/`~/.codex` **behind a dev flag**. Never commit real data; never use it in automated tests (`TESTING.md` §8). Prefer generating fixtures via the anonymization tool.

## 6. Definition of done (per change)
- Tests added/updated (incl. a regression fixture for any bug).
- `clippy`/`fmt`/`lint`/`typecheck` clean; relevant tests pass (report exactly what ran).
- Docs updated if architecture/schema/adapter/UX/security/IPC changed (AGENTS.md rule).
- No new outbound network; no secret leakage; privacy guards green.

## 7. For AI coding agents
Read `AGENTS.md` first — it encodes the constraints, the docs you must keep in sync, and what **not** to do.
