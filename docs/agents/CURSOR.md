# Agent: Cursor

> **Status: V1, experimental/best-effort.** Location **inspected directly**; schema is **INFERENCE** (opaque, undocumented, volatile). Do not let Cursor fragility affect other adapters. Cross-agent context: `docs/research/AGENT_STORAGE_FORMATS.md` *(internal)*.

## 1. Installation detection (FACT)
- Presence of `~/Library/Application Support/Cursor/` and `~/.cursor/`.
- Chat/composer data in the VS Code-style SQLite DB:
  - Global: `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb` (**observed ~532 MB**, plus `-wal`/`-shm`, `.backup`).
  - Per-workspace: `~/Library/Application Support/Cursor/User/workspaceStorage/<hash>/state.vscdb`.

## 2. Format (FACT location / INFERENCE schema)
- **FACT (direct schema inspection, 2026-08-10).** The observed DB contains `ItemTable(key TEXT UNIQUE, value BLOB)`, `cursorDiskKV(key TEXT UNIQUE, value BLOB)`, and `composerHeaders(composerId PK, workspaceId, createdAt, lastUpdatedAt, isArchived, isSubagent, recency, checkpointAt, value TEXT)`.
- **INFERENCE.** Chat/"composer" sessions are stored as JSON blobs under app-specific keys (historically things like `composer.composerData`, `workbench.panel.aichat.*`, `aiService.*`). Exact keys are **undocumented, versioned, and have changed repeatedly**; there is no stable public schema. Community exporters exist but break across Cursor releases — use them only as *hints*, never as a contract.

## 3. Parser strategy (planned, INFERENCE)
- For the live DB, use SQLite `mode=ro`, WAL-aware reads, and a bounded busy timeout, or create a consistent temporary snapshot through the SQLite backup API. `immutable=1` is valid only for a proven closed/static copy; using it on a live WAL database can ignore changes and violate its contract.
- Enumerate candidate keys, JSON-parse values defensively, and map recognizable chat/composer structures → `Message`/`ToolCall`. Everything is try/catch; unknown shapes → skip with a note.
- Resolve repo/worktree from any workspace/path hints in the blob or the `workspaceStorage/<hash>` mapping; fall back to "No repository."
- Isolate the whole adapter behind a panic boundary so a schema change can't crash Lore.

## 4. Edge cases & risks
- **R3 (High):** opaque + huge + live-written. Highest-maintenance adapter; expect breakage on Cursor updates.
- Large blobs → stream/limit; don't load the whole 532 MB DB into memory.
- Capability profile (INFERENCE): `messages` likely; `tool_calls` maybe; `token_usage`/`git_context`/`message_tree` likely absent → UI/search degrade accordingly.

## 5. Recommendation
Ship **after** V0 as clearly-labeled **beta**, opt-in per Agent settings, with a prominent "format may change" note. Prefer contributing a robust exporter or reading Cursor's own types if/when documented. Prioritize breadth via the **SpecStory Markdown fallback adapter** (which already captures Cursor) before investing heavily in raw `state.vscdb` parsing.

## 6. Test fixtures
- Synthetic `state.vscdb` with a known-shape composer blob.
- A blob with an unrecognized/newer shape (must degrade, not crash).
- A live-DB simulation (WAL present, concurrent writer) validating read-only WAL behavior or backup snapshots; a separate closed-copy test may use `immutable=1`.
