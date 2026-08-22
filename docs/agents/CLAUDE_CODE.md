# Agent: Claude Code

> Primary target. Schema below was **inspected directly** on macOS (2026-08-10) — keys/types extracted programmatically, no user content copied. Cross-agent context: `docs/research/AGENT_STORAGE_FORMATS.md` *(internal)*. Adapter contract: `docs/architecture/AGENT_ADAPTERS.md`. Tags: **FACT** / **INFERENCE**.

## 1. Installation detection
- **FACT.** By default, presence of `~/.claude/projects/` with `<encoded-project>/<uuid>.jsonl`; `CLAUDE_CONFIG_DIR` relocates the whole storage root and must be honored.
- User-configured custom source roots: added via Settings folder picker and persisted in `setting` under `agent_roots.claude-code` (M5).
- Version: read from any session event's `version` field, and/or `~/.claude/version.json`.
- Config signals: `~/.claude/settings.json`, `~/.claude/CLAUDE.md`, `~/.claude/skills/`, `~/.claude/agents/`, `~/.claude/plugins/`.

## 2. Storage locations (FACT)
| Path | Contents |
|---|---|
| `~/.claude/projects/<encoded-cwd>/<sessionUuid>.jsonl` | **Session transcripts** (one event per line). Primary ingest target. |
| `~/.claude/projects/<encoded-cwd>/<sessionUuid>/subagents/agent-<id>.jsonl` | **Subagent transcripts** (one per subagent). |
| `~/.claude/projects/<encoded-cwd>/<sessionUuid>/subagents/workflows/<wf-id>/agent-<id>.jsonl` | **Workflow-tool subagent transcripts** (fan-out, one per run directory). |
| `~/.claude/history.jsonl` | Global prompt history: `{display, pastedContents, timestamp, project, sessionId}` |
| `~/.claude/sessions/*.json` | UI/session sidecars (small) |
| `~/.claude/session-env/`, `shell-snapshots/`, `todos/` | Session-scoped env, shell snapshots, todo state |
| `~/.claude/CLAUDE.md`, `settings.json`, `skills/`, `agents/`, `plugins/` | Config + knowledge (not transcripts) |

Discovery walks the entire `projects` subtree for `*.jsonl` files — there is no fixed depth budget, because upstream has added transcript nesting levels before (subagents, then workflow subagents) and may again. Symlinked directories are not followed, so a symlink cycle cannot make the walk unbounded.

`<encoded-project>` = working-directory path with **every non-alphanumeric character** replaced by `-` (not only `/`). It is lossy, so do **not** decode it for identity; use per-event `cwd` and retain path aliases.

Official Claude Code defaults to deleting transcripts after 30 days (`cleanupPeriodDays` is configurable), can suppress persistence, and can move the root with `CLAUDE_CONFIG_DIR`. Lore discovery must therefore treat logs as mutable/perishable inputs rather than a guaranteed recovery source.

## 3. Session JSONL schema (FACT)

Each line is a JSON object with a `type`. Direct inspection observed `user`, `assistant`, `attachment`, `queue-operation`, `last-prompt`, `custom-title`, `ai-title`, `system`, `mode`, and `pr-link`. Unknown/future types remain expected.

**Common envelope (user/assistant):**
`uuid`, `parentUuid`, `sessionId`, `type`, `timestamp`, `cwd`, `gitBranch`, `version`, `userType`, `isSidechain`, `entrypoint`, `message`.
- `user` adds: `promptId`, `promptSource`, `origin`, `permissionMode`.
- `assistant` adds: `requestId`, `effort`.

**`message` object:**
- user: `{ role: "user", content }`, where `content` was observed as either a typed-block array **or a string**
- assistant: `{ role, id, model, content, usage, stop_reason, stop_details, stop_sequence, type, diagnostics }`

**`content` blocks (typed):**
| block | keys |
|---|---|
| `text` | `type, text` |
| `thinking` | `type, thinking, signature` (signature may be present; preserve as metadata) |
| `tool_use` | `type, id, name, input, caller` |
| `tool_result` | `type, tool_use_id, content, is_error` |

**`usage` keys:** `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`, `cache_creation`, `service_tier`, `server_tool_use`, `iterations`, `speed`, `inference_geo`.

**Non-message lines:**
- `custom-title`: `{customTitle, sessionId, type}` · `ai-title`: `{aiTitle, sessionId, type}` → session title.
- `last-prompt`: `{lastPrompt, leafUuid, sessionId, type}`.
- `queue-operation`: `{operation, content, sessionId, timestamp, type}`.
- `attachment`: envelope + `attachment` object.
- `mode`: `{mode, sessionId, type}`.
- `pr-link`: `{prNumber, prRepository, prUrl, sessionId, timestamp, type}`.
- `system`: subtype-dependent operational metadata (observed fields include `subtype`, `level`, hook/retry/error fields, cwd/gitBranch, uuid/parentUuid). Preserve known metadata without treating it as a chat message; unknown subtypes remain additive.

## 4. Illustrative record shapes (synthetic, structure only)
```jsonc
// user turn
{ "type":"user","uuid":"…","parentUuid":"…","sessionId":"…","timestamp":"…",
  "cwd":"/Users/x/proj","gitBranch":"billing","version":"1.x","isSidechain":false,
  "message":{"role":"user","content":[{"type":"text","text":"…"}]} }

// assistant turn with a tool call
{ "type":"assistant","uuid":"…","parentUuid":"…","sessionId":"…","requestId":"…",
  "cwd":"/Users/x/proj","gitBranch":"billing","effort":"…",
  "message":{"role":"assistant","model":"claude-…","stop_reason":"tool_use",
    "usage":{"input_tokens":41000,"output_tokens":600,"cache_read_input_tokens":38000,"…":0},
    "content":[{"type":"thinking","thinking":"…"},
               {"type":"tool_use","id":"toolu_…","name":"Edit","input":{"file_path":"…"},"caller":"…"}]}}

// tool result (usually as a user-role message referencing tool_use_id)
{ "type":"user","message":{"role":"user","content":[
   {"type":"tool_result","tool_use_id":"toolu_…","is_error":false,"content":"…"}]}}
```

## 5. Mapping to Lore's model
| Claude field | Lore |
|---|---|
| `sessionId` | `AgentSession.native_session_id` |
| `cwd` (per event) | `SessionSegment.cwd` → repo/worktree resolution; start a segment when it changes |
| `gitBranch` | `GitObservation{source:agent_recorded}.branch` |
| `version` | `AgentSession.agent_version` (fixture selection) |
| `uuid`/`parentUuid` | `Message.native_uuid` / `parent_id` (tree) |
| `isSidechain` | `Message.is_sidechain` |
| ordered content blocks | one Message + ordered `MessagePart`s; preserve mixed text/thinking/tool blocks |
| `tool_use` | `ToolCall{name,input}` |
| `tool_result` | `ToolCall.output_text`,`is_error`; derive `FileEvent` for Edit/Write/etc. |
| `usage.*` | per-turn tokens on `Message`; summed on `AgentSession` |
| `custom-title`/`ai-title` | `AgentSession.title`; when absent, fall back to the first meaningful user request |

## 6. Parser strategy
- **Stream line-by-line**; push rows to `IngestSink`; checkpoint byte offset in `ingest_state` (crash-safe resume; sessions reach multiple MB — one observed at 2.9 MB).
- Build the message tree from `uuid`/`parentUuid`; keep unresolved native parent ids until later rows arrive; keep `seq` from file order as total order.
- Derive `FileEvent`s from tool calls whose `name` ∈ {Edit, Write, MultiEdit, NotebookEdit, …} using `input.file_path` (+ diff if present in result).
- `caller` distinguishes tool invokers (main vs subagent) — pair with `isSidechain`.
- Compute per-session token/cost sums from assistant `usage`.

## 7. Edge cases & risks
- **R1 — lossy dir name:** never rely on `<encoded-cwd>`; the authoritative cwd is inside each event.
- **Retention/config:** discover the default and `CLAUDE_CONFIG_DIR`; surface the 30-day default cleanup risk and do not promise a future re-scan can recover deleted logs.
- **R5 — partial final line:** the agent may be mid-write; parse to last complete line, re-ingest on FSEvents change.
- **R2 — version drift:** newer fields appear (`effort`, `inference_geo`, `speed`); treat unknown keys/blocks/types as additive; never hard-fail.
- **R8 — huge sessions:** stream; offload very large tool outputs to `blobs/`.
- **Sidechains:** subagent transcripts interleave; keep them attributed but visually distinguishable.
- **Thinking privacy:** `thinking` content is sensitive; **not indexed by default** (opt-in) and never exported without redaction awareness.

## 8. Test fixtures to build (anonymized)
- Minimal 2-turn session (user→assistant text).
- Session with tool_use + tool_result (success + `is_error:true`).
- Session with `thinking` blocks.
- Session with sidechain (`isSidechain:true`, subagent `caller`).
- Multi-MB session (perf).
- Truncated final line (partial-parse recovery).
- Unknown future `type`/block (forward-compat / degrade to `partial`).
- Session with a planted fake secret in tool output (secret-scanner test).
- Custom-title and ai-title variants.
- String-valued user content; `mode`/`pr-link`/`system`; thinking `signature` preservation.
See `docs/development/TESTING.md` for the anonymization pipeline.

## Sources

- Direct keys/types inspection on macOS (2026-08-10), with no content copied.
- Official session storage/configuration/retention and `/branch`/`--fork-session`: https://code.claude.com/docs/en/sessions
