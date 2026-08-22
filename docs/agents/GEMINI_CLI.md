# Agent: Gemini CLI

> **Status: V0.5 target.** The local `~/.gemini/tmp` base was inspected, but no live `chats/` session corpus was present; session/checkpoint locations below come from **official docs** and field-level schema remains unverified. Cross-agent context: `docs/research/AGENT_STORAGE_FORMATS.md` *(internal)*.

## 1. Installation detection (FACT)
- Presence of `~/.gemini/` with `config/`, `projects.json`, `GEMINI.md`, and `tmp/<project_hash>/`.
- Note: `~/.gemini/` on the inspected machine also holds `antigravity*` dirs (a related IDE) — the CLI's session data is under `tmp/<project_hash>/`, not those.

## 2. Storage locations (FACT / docs)
| Path | Contents |
|---|---|
| `~/.gemini/tmp/<project_hash>/chats/` | Automatically saved session history: conversation, tools, token usage, and available reasoning summaries |
| `~/.gemini/tmp/<project_hash>/checkpoints/` | Checkpoint conversation/tool JSON, **only when checkpointing is enabled** |
| `~/.gemini/history/<project_hash>/` | Optional checkpoint shadow Git repository, separate from the user's repo |
| `~/.gemini/config/`, `projects.json`, `GEMINI.md` | Config / project registry / instructions |

- `<project_hash>` is documented as a unique identifier based on the project root. The exact hash algorithm/mapping remains **INFERENCE** until pinned source/fixtures confirm it.
- Checkpoint filenames encode timestamp + target file + tool (e.g. `<ts>-<file>-<tool>`).

## 3. Schema (docs; verify)
- `chats/`: exact file names/JSON fields remain **unverified** on the inspected machine and must be confirmed against a pinned live version/source before implementation.
- Checkpoints: when explicitly enabled, JSON state pairs with a shadow-git commit. Checkpointing is **disabled by default**, so it cannot be assumed as a normal FileEvent/Git evidence source.

## 4. ⚠ Critical risk: 30-day auto-delete (FACT / docs)
- Default policy **auto-deletes session data after 30 days**. Lore may become the *only* surviving copy. Implications:
  - **Ingest eagerly**; watch `~/.gemini/tmp` closely.
  - Surface this to the user ("Gemini history is deleted after 30 days — Lore has preserved N sessions").
  - Consider (opt-in) suggesting the user raise the retention setting.

## 5. Parser strategy (planned)
- Discover session files under `chats/` and parse only after their pinned schema is verified.
- When checkpointing is present, use checkpoint metadata/shadow Git as separately labeled optional evidence; absence is normal.
- Resolve repo by matching `<project_hash>` to known roots; else "No repository."

## 6. Capability profile (INFERENCE)
`messages` ✅ (docs), `tool_calls` ✅ (docs), `token_usage` ✅ (docs), `file_events` ⚠ only with optional checkpoints, `git_context` ⚠ optional shadow Git and **not** the user's branch/commit, `message_tree` ? . Exact schema/linearity remains unverified.

## 7. Test fixtures
- Pinned `chats/` session with turns, tools, tokens, and reasoning-summary variants.
- Checkpointing disabled (normal absence) and enabled checkpoint + shadow-git pair.
- A `<project_hash>` that does/doesn't resolve to a known repo.
- Aged data near the 30-day boundary (retention-warning UX).

## Sources
- Checkpointing: https://google-gemini.github.io/gemini-cli/docs/cli/checkpointing.html
- Sessions/retention: https://geminicli.com/docs/cli/session-management/
