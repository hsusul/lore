# Agent: OpenCode

> **Status: V0.5 target.** Sourced from OpenCode docs, the current canonical repository, DeepWiki, and ccusage. **Not installed on the inspection machine** → exact storage fields still need confirmation against a pinned install/source before implementation. Tags: **FACT (primary/secondary as stated)** / **INFERENCE**. Cross-agent context: `docs/research/AGENT_STORAGE_FORMATS.md` *(internal)*.

> **Canonical repository:** `anomalyco/opencode` (the old `sst/opencode` URL redirects there). Pin an exact release/commit before implementation because the persistence format evolves; treat old org references only as historical source links.

## 1. Installation detection (FACT, secondary)
- Data dir: **`~/.local/share/opencode/`** (default), overridable via **`OPENCODE_DATA_DIR`** (accepts one dir or a comma-separated list).
- Config/instructions: **`~/.config/opencode/`** (global), plus project-local **`.opencode/`** (agents as Markdown+YAML frontmatter, `modes/`, `commands/`, `plugins/`) and `AGENTS.md`/`CLAUDE.md` discovery (global → project root → nearby).
- Presence of `~/.local/share/opencode/opencode.db` and/or `~/.local/share/opencode/storage/` confirms an install.

## 2. Storage architecture — **three layers** (FACT, secondary)
OpenCode uses a hybrid persistence model (`<data>` = `~/.local/share/opencode/`):

| Layer | Location | Holds |
|---|---|---|
| **SQLite (Drizzle ORM)** | `<data>/opencode.db` | relational: sessions, messages, **parts**, project metadata |
| **File system (JSON)** | `<data>/storage/*` | large objects & historical artifacts (see tree below) |
| **Internal git (snapshots)** | `<data>/snapshot/<projectID>/<hash>` | file-level undo; a **separate internal git repo** so agent tracking doesn't pollute the user's `.git` |

**JSON storage tree (`<data>/storage/`):**
```
storage/
  session/{projectHash}/{sessionID}.json         # session index/metadata
  message/{sessionID}/msg_{messageID}.json       # one file per message
  part/…                                          # message "parts": text/tool/file snapshots
  session_diff/…                                  # session state diffs (split out for perf, Migration 2)
  session-metadata/{projectID}/{sessionID}.json  # arbitrary session metadata
```
Storage keys are hierarchical string arrays mapping directly to the filesystem (`read/update/list(prefix)`), with atomic read-modify-write.

## 3. Key facts for Lore (FACT, secondary)
- **Session identity:** each session has a `SessionID`, belongs to a single `ProjectID`, and holds an ordered sequence of messages. **`projectHash` is derived from Git-root discovery** (Migration 1 reorganized sessions into per-project subdirs) → useful for repo attribution.
- **Message hierarchy:** parent/subagent links exist (ccusage renders parent→subagent trees) → maps to `Message.parent_id` + `is_sidechain`.
- **Tokens stored, cost not:** message files store `cost: 0`; **token counts are present** and cost is derived (ccusage uses LiteLLM pricing). So Lore gets `token_usage` but must compute cost itself.
- **Model names stored** (incl. variants like `gemini-3-pro-high`).
- **"Parts"** decompose a message into text/tool/file-snapshot units → maps cleanly to Lore `ToolCall`/`FileEvent`.
- **Internal snapshot git** = a per-project undo repo (analogous to Gemini's optional shadow repo) → a potential separately labeled `FileEvent`/`GitObservation` source.

## 4. Stability & retention (FACT, secondary)
- **Persists indefinitely** — there is **no default auto-cleanup**; session storage **grows unboundedly** (open issues request an `opencode session prune`). Contrast Gemini's 30-day TTL: OpenCode data is durable (good for Lore) but can get large.
- **Schema evolves** (documented migrations reorganized session dirs and split `session_diff/`). Hybrid SQLite+JSON is comparatively new → treat as **Low–Med stability**; pin + fixture per version.

## 5. Parser strategy (DECISION)
- **Read the JSON storage layer, not primarily the SQLite** — the `storage/*` JSON is the documented "historical artifact" surface and is easier to parse read-only without touching a live Drizzle DB. Use SQLite (`opencode.db`, open read-only) only if a needed field lives solely there.
- **Confirm exact field names against `anomalyco/opencode` source/types** at the pinned release; define the mapping from authoritative serializers, per ADR-0003.
- Resolve repo via `projectHash` (git-root-derived) → Lore repo identity; else "No repository."
- Pair message `part`s → `ToolCall`/`FileEvent`; build the parent/subagent tree; sum tokens per session; derive cost.

## 6. Capability profile (FACT-leaning, verify)
`messages` ✅ · `tool_calls` ✅ (parts) · `token_usage` ✅ (cost derived) · `model_name` ✅ · `message_tree` ✅ (parent/subagent) · `file_events` ⚠ (via parts + snapshot git) · `git_context` ⚠ (projectHash from git root; internal snapshot git — user branch/commit availability **TBD**) · `durations` ? · Stability **Low–Med** · Difficulty **Med**.

## 7. Before implementing — checklist
- [ ] Pin `anomalyco/opencode` to a release/commit; install; capture anonymized fixtures.
- [ ] Confirm `session`/`message`/`part` JSON field names from source; confirm what git identity (branch/commit) is recoverable.
- [ ] Decide JSON-layer vs SQLite for each field; open SQLite read-only if used.
- [ ] Map → Lore model; set `capabilities()`; add version-keyed fixtures.
- [ ] Upgrade FACT(secondary) → FACT(inspected) here with the pinned version noted.

## 8. Interim breadth
Until a native adapter exists, the **SpecStory Markdown fallback adapter** may already capture OpenCode sessions — cheaper breadth than a bespoke parser.

## Sources
- OpenCode docs: https://opencode.ai/docs/config/ · SDK: https://opencode.ai/docs/sdk/
- Canonical source: https://github.com/anomalyco/opencode
- DeepWiki (historical `sst/opencode` indexing; secondary): https://deepwiki.com/sst/opencode/2.9-storage-and-database · https://deepwiki.com/sst/opencode/2.1-session-management
- ccusage OpenCode data source (paths, `cost:0`, tokens, parent/subagent): https://ccusage.com/guide/opencode/
- Unbounded-growth / no auto-cleanup (issue reports, secondary): https://github.com/anomalyco/opencode/issues/4980 · https://github.com/anomalyco/opencode/issues/22110
- Authoritative schema = pinned `anomalyco/opencode` source before implementing.
