# Docs Index — canonical sources of truth

> Concept → the **one** document that owns it. Avoid duplicating truth: if two docs describe the same thing, one is canonical and the other links to it. When you change a subsystem, update its canonical doc (see `AGENTS.md` → "Documentation you must keep in sync").

## Concept → canonical document

| Concept | Canonical doc |
|---|---|
| What Lore is / vision / principles | [product/VISION.md](product/VISION.md) |
| Product requirements, V0 scope, killer feature, JTBD | [product/PRD.md](product/PRD.md) |
| Milestones / implementation plan / performance targets | [product/ROADMAP.md](product/ROADMAP.md) |
| What we will **not** build | [product/NON_GOALS.md](product/NON_GOALS.md) |
| System architecture, modules, IPC contract (including live/custom source roots, user-defined folders, local backups, and detail-only degraded-parse diagnostics), process model | [architecture/ARCHITECTURE.md](architecture/ARCHITECTURE.md) |
| **Database schema / source identity / message parts / relationships / indexes** | [architecture/DATA_MODEL.md](architecture/DATA_MODEL.md) |
| **Agent adapter interface / capabilities / robustness rules** | [architecture/AGENT_ADAPTERS.md](architecture/AGENT_ADAPTERS.md) |
| **Git identity evidence, worktrees, observation provenance, lifecycle** | [architecture/GIT_INTEGRATION.md](architecture/GIT_INTEGRATION.md) |
| Search (FTS5 → hybrid), tokenizer, ranking, filters | [architecture/SEARCH.md](architecture/SEARCH.md) |
| **Skill/knowledge extraction (git-evidenced, privacy modes)** | [architecture/SKILL_EXTRACTION.md](architecture/SKILL_EXTRACTION.md) |
| Threat model, secrets, privacy contract | [architecture/SECURITY.md](architecture/SECURITY.md) |
| **Secret-scanner ruleset (patterns, entropy, redaction, tests)** | [architecture/SECRET_SCANNING.md](architecture/SECRET_SCANNING.md) |
| Local-first constraints & the network-boundary | [architecture/LOCAL_FIRST.md](architecture/LOCAL_FIRST.md) |
| Visual/interaction language, tokens, keyboard model | [design/DESIGN_SYSTEM.md](design/DESIGN_SYSTEM.md) |
| Navigation / IA / screen map | [design/INFORMATION_ARCHITECTURE.md](design/INFORMATION_ARCHITECTURE.md) |
| Screen wireframes | [design/WIREFRAMES.md](design/WIREFRAMES.md) |
| Build/dev setup, conventions, DoD | [development/DEVELOPMENT.md](development/DEVELOPMENT.md) |
| Testing strategy, fixtures, guards | [development/TESTING.md](development/TESTING.md) |
| Signing, notarization, update, distribution | [development/RELEASES.md](development/RELEASES.md) |
| Competitive landscape & differentiation | [research/COMPETITIVE_LANDSCAPE.md](research/COMPETITIVE_LANDSCAPE.md) |
| Per-competitor deep notes | [research/COMPETITOR_NOTES.md](research/COMPETITOR_NOTES.md) |
| **Agent on-disk storage formats (cross-agent)** | [research/AGENT_STORAGE_FORMATS.md](research/AGENT_STORAGE_FORMATS.md) |
| OSS to reuse/learn from | [research/OPEN_SOURCE_PROJECTS.md](research/OPEN_SOURCE_PROJECTS.md) |
| Claude Code parsing (facts + fixtures) | [agents/CLAUDE_CODE.md](agents/CLAUDE_CODE.md) |
| Codex parsing (facts + fixtures) | [agents/CODEX.md](agents/CODEX.md) |
| Cursor parsing (experimental) | [agents/CURSOR.md](agents/CURSOR.md) |
| Gemini CLI parsing | [agents/GEMINI_CLI.md](agents/GEMINI_CLI.md) |
| OpenCode parsing (unverified) | [agents/OPENCODE.md](agents/OPENCODE.md) |
| Key decisions (with rationale) | [decisions/](decisions/) (ADR-0001…0006) |

## Decision records (ADRs)
| ADR | Topic |
|---|---|
| [0001](decisions/0001-desktop-framework.md) | Desktop framework → Tauri |
| [0002](decisions/0002-local-database.md) | Local database → SQLite |
| [0003](decisions/0003-agent-adapter-model.md) | Agent adapter model → trait + capabilities |
| [0004](decisions/0004-search-strategy.md) | Search → FTS5 now, hybrid later |
| [0005](decisions/0005-local-first-security.md) | Local-first & security posture |
| [0006](decisions/0006-git-evidence-and-repository-identity.md) | Git evidence provenance & repository identity |

## Rules to prevent drift
1. **One owner per concept** (table above). Other docs *link*, don't restate.
2. **Change the code → change its canonical doc** in the same PR (AGENTS.md enforces).
3. **Cross-link, don't copy.** Shared facts (e.g. storage formats) live once (research/agents docs) and are referenced by architecture docs.
4. **Mark status** (FACT/INFERENCE/OPINION; verified/unverified) so readers know what's load-bearing.
