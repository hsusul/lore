# CLAUDE.md

Claude Code / Claude-specific notes. **The canonical contributor guide is [`AGENTS.md`](AGENTS.md)** — read it first. This file only adds Claude-specific pointers; it deliberately does **not** duplicate AGENTS.md.

## Orientation
- Start: [`AGENTS.md`](AGENTS.md), then [`docs/DOCS_INDEX.md`](docs/DOCS_INDEX.md).
- Status: **active implementation**. The Rust core, Claude Code and Codex adapters, storage/search/Git paths, Tauri shell, and React UI exist; packaging and V0 acceptance work remain. Use `docs/product/ROADMAP.md` *(internal)* for current status. Don't claim a gate passed without running it.

## Especially relevant when working here
- **Lore parses Claude Code's own sessions.** The authoritative schema (verified by direct inspection) is [`docs/agents/CLAUDE_CODE.md`](docs/agents/CLAUDE_CODE.md) — read it before touching the Claude adapter. Storage: `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl`.
- Never rely on the lossy `<encoded-cwd>` dir name for identity; use each event's `cwd`.
- `thinking` blocks are sensitive: not indexed by default, never exported without redaction awareness.
- Test the parser on **anonymized fixtures**, never on real `~/.claude` history in CI (`docs/development/TESTING.md`).

## Guardrails (from AGENTS.md — the ones easiest to trip)
- No network capability in archive modules; the separate update check is off by default and explicit. No LLM calls in V0.
- Read-only on all agent files.
- Keep agent-specific code inside an adapter.
- If you change schema/adapter/IPC/git/search/security/UX, update the canonical doc in the same change (map: `docs/DOCS_INDEX.md`).

## Naming note
**SpecStory ships a feature called "Lore"** (sessions→skills). **Decided (2026-08-10): keep "Lore" everywhere** and differentiate via git-evidenced skills (`RESEARCH_SUMMARY.md` *(internal)* §9.1). A trademark/SEO check is still owed before public launch.
