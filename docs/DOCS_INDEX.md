# Docs Index — canonical sources of truth

> Concept → the **one** document that owns it. Avoid duplicating truth: if two docs describe the same thing, one is canonical and the other links to it. When you change a subsystem, update its canonical doc (see `AGENTS.md` → "Documentation you must keep in sync").

> **Published vs internal.** This repository is public, and it publishes the docs a contributor needs in order to build correctly: architecture, per-agent parsing, and development/testing/release process. Product strategy, unshipped design, decision rationale, and competitor research are kept in the maintainer's working tree and are **not** in this repository. They are listed below without links, marked *(internal)*, so the map stays honest — a reference you cannot open is internal by design, not a broken link.

## Concept → canonical document

### Published in this repository

| Concept | Canonical doc |
|---|---|
| System architecture, modules, IPC contract (including live/custom source roots, user-defined folders, local backups, and detail-only degraded-parse diagnostics), process model | [architecture/ARCHITECTURE.md](architecture/ARCHITECTURE.md) |
| **Database schema / source identity / message parts / relationships / indexes** | [architecture/DATA_MODEL.md](architecture/DATA_MODEL.md) |
| **Agent adapter interface / capabilities / robustness rules** | [architecture/AGENT_ADAPTERS.md](architecture/AGENT_ADAPTERS.md) |
| **Git identity evidence, worktrees, observation provenance, lifecycle** | [architecture/GIT_INTEGRATION.md](architecture/GIT_INTEGRATION.md) |
| Search (FTS5 → hybrid), tokenizer, ranking, filters | [architecture/SEARCH.md](architecture/SEARCH.md) |
| **Skill/knowledge extraction (git-evidenced, privacy modes)** | [architecture/SKILL_EXTRACTION.md](architecture/SKILL_EXTRACTION.md) |
| Threat model, secrets, privacy contract, local backups, and recovery | [architecture/SECURITY.md](architecture/SECURITY.md) |
| **Secret-scanner ruleset (patterns, entropy, redaction, tests)** | [architecture/SECRET_SCANNING.md](architecture/SECRET_SCANNING.md) |
| Local-first constraints & the network-boundary | [architecture/LOCAL_FIRST.md](architecture/LOCAL_FIRST.md) |
| Claude Code parsing (facts + fixtures) | [agents/CLAUDE_CODE.md](agents/CLAUDE_CODE.md) |
| Codex parsing (facts + fixtures) | [agents/CODEX.md](agents/CODEX.md) |
| Cursor parsing (experimental) | [agents/CURSOR.md](agents/CURSOR.md) |
| Gemini CLI parsing | [agents/GEMINI_CLI.md](agents/GEMINI_CLI.md) |
| OpenCode parsing (unverified) | [agents/OPENCODE.md](agents/OPENCODE.md) |
| Build/dev setup, conventions, DoD | [development/DEVELOPMENT.md](development/DEVELOPMENT.md) |
| Testing strategy, fixtures, guards | [development/TESTING.md](development/TESTING.md) |
| Signing, notarization, update, distribution | [development/RELEASES.md](development/RELEASES.md) |

### Internal — not published in this repository

These remain canonical for their concepts; they are simply not public. Contributors do not need them to build, and nothing in the published docs depends on reading them.

| Concept | Canonical doc |
|---|---|
| What Lore is / vision / principles | `docs/product/VISION.md` *(internal)* |
| Product requirements, V0 scope, killer feature, JTBD | `docs/product/PRD.md` *(internal)* |
| Milestones / implementation plan / performance targets | `docs/product/ROADMAP.md` *(internal)* |
| What we will **not** build | `docs/product/NON_GOALS.md` *(internal)* |
| Visual/interaction language, tokens, keyboard model | `docs/design/DESIGN_SYSTEM.md` *(internal)* |
| Navigation / IA / screen map | `docs/design/INFORMATION_ARCHITECTURE.md` *(internal)* |
| Screen wireframes | `docs/design/WIREFRAMES.md` *(internal)* |
| Competitive landscape & differentiation | `docs/research/COMPETITIVE_LANDSCAPE.md` *(internal)* |
| Per-competitor deep notes | `docs/research/COMPETITOR_NOTES.md` *(internal)* |
| **Agent on-disk storage formats (cross-agent)** | `docs/research/AGENT_STORAGE_FORMATS.md` *(internal)* |
| OSS to reuse/learn from | `docs/research/OPEN_SOURCE_PROJECTS.md` *(internal)* |
| Executive research brief / open questions | `RESEARCH_SUMMARY.md` *(internal)* |
| Key decisions (with rationale) | `docs/decisions/` ADR-0001…0006 *(internal)* |

Internal ADRs: 0001 desktop framework → Tauri · 0002 local database → SQLite · 0003 agent adapter model → trait + capabilities · 0004 search → FTS5 now, hybrid later · 0005 local-first & security posture · 0006 git evidence provenance & repository identity.

## Rules to prevent drift
1. **One owner per concept** (tables above). Other docs *link*, don't restate.
2. **Change the code → change its canonical doc** in the same PR (AGENTS.md enforces).
3. **Cross-link, don't copy.** Shared facts (e.g. storage formats) live once and are referenced by architecture docs.
4. **Mark status** (FACT/INFERENCE/OPINION; verified/unverified) so readers know what's load-bearing.
5. **Never move an internal doc into the published set without re-reading it for strategy or third-party content.** The split above is deliberate, not incidental.
