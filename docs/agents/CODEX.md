# Agent: Codex CLI

> Primary target. `sessions/` rollout schema **inspected directly** on macOS (2026-08-10). Cross-agent context: `docs/research/AGENT_STORAGE_FORMATS.md` *(internal)*. Tags: **FACT** / **INFERENCE**.
>
> ⚠️ The inspected machine also has a heavily-customized `~/.codex/` (extra sqlite DBs: `logs_2.sqlite`, `state_5.sqlite`, `memories_1.sqlite`, plus `ambient-suggestions/`, `computer-use/`, `pets/`). **These are NOT the standard OpenAI Codex CLI surface — Lore must NOT depend on them.** The `~/.codex/sessions/` rollout JSONL below matches the documented Codex CLI and is the target.

## 1. Installation detection
- **FACT.** Presence of `~/.codex/sessions/` containing `YYYY/MM/DD/rollout-*.jsonl`; `CODEX_HOME` relocates the default storage root.
- User-configured custom source roots: added via Settings folder picker and persisted in `setting` under `agent_roots.codex` (M5).
- Version: `session_meta.payload.cli_version` (e.g. `0.133.0-alpha.1`); also `~/.codex/config.toml`, `~/.codex/AGENTS.md`, `~/.codex/skills/`.

## 2. Storage locations (FACT)
| Path | Contents |
|---|---|
| `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO8601>-<uuid>.jsonl` | **Session rollouts** (one event per line). Primary target. Date-sharded. |
| `~/.codex/archived_sessions/rollout-*.jsonl` | Archived rollouts (same format) |
| `~/.codex/config.toml` | Config |
| `~/.codex/AGENTS.md`, `skills/` | Instructions / skills |

## 3. Rollout JSONL schema (FACT)

Every line: `{ type, timestamp, payload }`. Direct inspection observed top-level `session_meta`, `event_msg`, `response_item`, `turn_context`, `compacted`, `world_state`, and `inter_agent_communication_metadata`. Unknown types remain expected.

**`session_meta.payload`:** commonly includes `id`, `timestamp`, `cwd`, `originator`, `cli_version`, `source`, `thread_source`, `model_provider`, `base_instructions`, and `dynamic_tools`; newer/local variants also carried fields such as `context_window`, `history_mode`, and multi-agent metadata. `git` is **optional and variably populated**: it may be absent, `{}`, branch-only, branch+commit, or branch+commit+repository URL.

In the inspected corpus, only 82 of 515 `session_meta` rows had all three git fields; 278 had no `git` key. The adapter must describe this as partial evidence, not a guaranteed best-in-class stamp.

**`turn_context.payload`:** observed fields include `cwd`, `model`, `effort`, `summary`, `approval_policy`, `sandbox_policy`, `permission_profile`, `collaboration_mode`, `personality`, `current_date`, `timezone`, `turn_id`, plus newer fields such as `file_system_sandbox_policy`, `approvals_reviewer`, and `workspace_roots`. Context can change between turns and maps to SessionSegments.

**`response_item.payload` (by `payload.type`):**
| type | keys |
|---|---|
| `message` | `role`, `content` |
| `reasoning` | `content`, `summary`, **`encrypted_content`** (opaque — never index/export) |
| `function_call` | `name`, `arguments`, `call_id` |
| `function_call_output` | `call_id`, `output` |
| `custom_tool_call` / `_output` | `name`,`input`,`status`,`call_id` / `call_id`,`output` |
| `tool_search_call` / `_output` | `arguments`,`execution`,`status` / `tools`,`execution` |
| `web_search_call` | `action`, `status` |

**`event_msg.payload` (by `payload.type`):**
| type | keys |
|---|---|
| `task_started` | `model_context_window`, `turn_id`, `started_at`, `collaboration_mode_kind` |
| `user_message` | `message`, `images`, `local_images`, `text_elements` |
| `agent_message` | `message`, `phase`, `memory_citation` |
| `agent_reasoning` | `text` |
| `thread_settings_applied` | `thread_settings` |
| **`patch_apply_end`** | `call_id`, **`changes` object keyed by path**, `status`, `stdout`, `stderr`, `success`, `turn_id` |
| `mcp_tool_call_end` | `invocation`, `result`, `duration`, `call_id` |
| `web_search_end` | `query`, `action`, `call_id` |
| `token_count` | `info`, `rate_limits` |
| `task_complete` | `last_agent_message`, `duration_ms`, `time_to_first_token_ms`, `turn_id` |
| `turn_aborted` | `reason`, `duration_ms`, `turn_id` |
| `context_compacted` | — |
| (`compacted` top-level) | `message`, `replacement_history` |

## 4. Illustrative record shapes (synthetic, structure only)
```jsonc
{ "type":"session_meta","timestamp":"…","payload":{
    "id":"019e…","cwd":"/Users/x/proj","cli_version":"0.133.0-alpha.1","source":"vscode",
    "model_provider":"openai","git":{"branch":"billing","commit_hash":"3ab9f1…","repository_url":"github.com/x/proj"}}}

{ "type":"response_item","timestamp":"…","payload":{
    "type":"function_call","name":"apply_patch","arguments":"{…}","call_id":"call_…"}}

{ "type":"event_msg","timestamp":"…","payload":{
    "type":"patch_apply_end","call_id":"call_…","success":true,
    "changes":{"billing/verify.ts":{"type":"update","unified_diff":"@@ synthetic patch @@"}},
    "stdout":"…","stderr":""}}

{ "type":"event_msg","timestamp":"…","payload":{
    "type":"token_count","info":{"input":41000,"output":600},"rate_limits":{…}}}
```
Observed `changes` values used `{type, content}` for some creates/writes and `{type, unified_diff, move_path?}` for patch/move cases. Line counts are **not stored as `added`/`removed` fields**; derive them by parsing `unified_diff` when present. `token_count.info` remains version-sensitive.

## 5. Mapping to Lore's model
| Codex | Lore |
|---|---|
| rollout uuid (filename / `session_meta.id`) | `AgentSession.native_session_id` |
| `session_meta.cwd` / `turn_context.cwd` | initial/per-turn `SessionSegment.cwd` |
| optional `session_meta.git.*` | `GitObservation{source:agent_recorded}` for fields actually present + identity hints |
| `cli_version` | `AgentSession.agent_version` |
| `turn_context.cwd/model` | SessionSegment context / per-turn model |
| `response_item.message` | Message + ordered MessageParts |
| first meaningful user request | fallback `AgentSession.title` (bootstrap/context messages skipped) |
| `response_item.reasoning` | reasoning MessageParts; `encrypted_content` → opaque_excluded Blob |
| `function_call` + `function_call_output` | `ToolCall{name,input,output}` paired by `call_id` |
| `patch_apply_end.changes[path]` | `FileEvent{path, patch_blob, source:agent_patch}`; line counts derived from unified diff |
| `token_count` / `task_complete.duration_ms` | session tokens / latency |

## 6. Parser strategy
- Stream line-by-line; a session is a **linear** event log (no parent tree) — order by file position/timestamp; assign `seq`.
- Pair tool calls by `call_id` (`function_call`↔`function_call_output`; `patch_apply_end` references its `call_id`).
- Preserve each `patch_apply_end.changes` value byte-faithfully, keyed by its object path; parse type/move/diff defensively and derive counts only from a valid diff.
- Emit only git fields actually present. Lore's repository observation at ingest is separate and never fills an `agent_recorded` row.
- Exclude `reasoning.encrypted_content` from text/FTS/export entirely.

## 7. Edge cases & risks
- **R2 — higher churn:** many `event_msg`/`response_item` subtypes and clearly-new ones (`tool_search_*`); key adapter behavior off `cli_version`; degrade unknown subtypes to notes.
- **Sparse session git:** absence is normal; do not mark the whole session partial merely because `session_meta.git` is absent.
- **R7 — encrypted regions:** opaque `encrypted_content` must never be indexed/exported.
- **Compaction:** `compacted`/`context_compacted` mean history was summarized/replaced mid-session — represent faithfully (a compaction marker), don't drop preceding messages silently.
- **Custom `.codex` variants:** ignore non-standard sqlite/state files; only parse `sessions/**/rollout-*.jsonl`.
- **Multi-provider:** `model_provider` may be non-openai; store it; don't assume OpenAI pricing when estimating cost.

## 8. Test fixtures to build (anonymized)
- Minimal session (`session_meta` + one message turn + `task_complete`).
- Session with `function_call`/`function_call_output` pair.
- `patch_apply_end.changes` map fixtures for create/content, update/unified_diff, move/move_path, and malformed/unknown values.
- `session_meta.git` absent, empty, branch-only, branch+commit, and full variants.
- Session with `reasoning.encrypted_content` (exclusion test).
- Session with `compacted`/`context_compacted`.
- Session with unknown future `payload.type` (degrade to `partial`).
- Non-openai `model_provider` (cost-estimation guard).
- Planted fake secret in `function_call_output` (secret scanner).
