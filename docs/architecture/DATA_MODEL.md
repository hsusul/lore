# Data Model

> Canonical normalized schema for every adapter. Any implementation change to these entities requires a migration, this document, generated IPC types, and tests. Tags: **DECISION** / **INFERENCE** / **OPINION**.

## 1. Design invariants

1. **Faithful event structure.** Ordered mixed content blocks, message trees, tool-call/result relationships, and opaque regions must survive normalization.
2. **Context can change within a session.** cwd, repository, worktree, model, and Git evidence attach to segments/turns, not only to the session header.
3. **Evidence never loses provenance.** Agent-recorded Git values and patches, Lore capture-time observations, and later verification remain separate.
4. **Identity is evidence-based.** Paths and root commits are signals, not universal repository identifiers.
5. **Ingest is idempotent and recoverable.** A source file may append, truncate, rewrite, move to an archive, or be copied; checkpoints never advance beyond committed normalized rows.
6. **Search is a projection.** Secret-redacted `SearchDocument` rows feed FTS; canonical normalized content does not double as the index contract.
7. **Large content has an explicit reference.** A field never says “may offload” without a `blob_id` and scan state.

SQLite-ish types are shown below. `id` is a Lore ULID unless noted; timestamps are UTC epoch ms. Agent-specific extras live in typed/versioned JSON only when no normalized field fits.

## 2. Relationship overview

```text
Agent 1─* AgentSession *─* SourceArtifact
                │
                ├─* SessionSegment *─0..1 Repository *─* RepositoryIdentityEvidence
                │         │                 └─* Worktree
                │         └─* GitObservation
                │
                ├─0..1 SessionFolder *─1 Folder
                │
                └─* Message ─* MessagePart
                         └─* ToolCall ─* FileEvent

Blob 1←* MessagePart / ToolCall / FileEvent / GitObservation
SearchDocument ─1 Search FTS row
SecretFinding → any persisted cleartext source by (source_kind, source_id, field)
Skill *─* evidence rows (V0.5+)
```

## 3. Source and ingest entities

### Setting — Lore-owned configuration

`Setting(key TEXT PK, value_json JSON, updated_at INT)` stores local application configuration:
- Custom adapter roots use `agent_roots.<agent_id>` with a JSON array of canonical absolute directory paths. They are configuration, not source evidence: removing one changes future discovery/watch coverage but never deletes `SourceArtifact` or archived session rows.
- Automatic backup configuration uses `backup.interval` (`"off" | "daily" | "weekly"`, defaulting to `"off"`), `backup.keep` (numeric retention count clamped to 1..100, defaulting to 7 snapshots), and `backup.last_at` (epoch ms timestamp of latest run).

Values remain local and contain no session content. Clearing archived content preserves settings so preferences, backup schedule, and configured source roots remain.

### Folder and SessionFolder — Lore-owned organization

`Folder(id TEXT PK, name TEXT, position INT, created_at INT, updated_at INT)` stores user-defined folders for organizing sessions. `id` is a 128-bit hex string. Folder names are validated and normalized on entry: unprintable ASCII control characters and invisible zero-width characters are rejected, whitespace is collapsed and trimmed, length is capped at 100 characters in storage (and 256 bytes at the IPC boundary), and blank input defaults to `"New folder"`.

`SessionFolder(session_id TEXT PK REFERENCES AgentSession(id), folder_id TEXT REFERENCES Folder(id), added_at INT)` tracks thread folder membership. Membership is mutually exclusive: `session_id` is the primary key so filing replaces any prior folder assignment. Deleting a folder unfiles its sessions (cascades) without deleting the session.

### Agent

| field | type | notes |
|---|---|---|
| id | TEXT PK | `claude-code`, `codex`, … |
| display_name | TEXT | |
| detected | BOOL | installation/root found |
| version | TEXT NULL | detected version |
| capabilities_json | JSON | adapter capability descriptor |

### SourceArtifact — one physical/logical input stream

| field | type | notes |
|---|---|---|
| id | ULID PK | stable Lore identity |
| agent_id | FK → Agent | |
| current_path | TEXT | latest observed path |
| native_file_id | TEXT NULL | platform file identity when available; hint, not sole identity |
| size / mtime | INT | latest stat |
| full_hash | TEXT NULL | content hash after a complete pass |
| prefix_hash | TEXT NULL | validates append-only resume |
| generation | INT | increments on rewrite/truncation |
| state | TEXT | `active | archived | missing | quarantined` |
| first_seen_at / last_seen_at | INT | |

`SourceArtifactPath(source_artifact_id, path, first_seen_at, last_seen_at)` retains move/archive aliases. Discovery matches by native session id plus hashes/file identity; path alone never defines a session. Ingest dedupe probes look up by `(agent_id, current_path)` and by `(agent_id, native_file_id, full_hash)` — both indexed (migration 0005) so re-ingest stays O(log n) in archive size.

#### Hash format evolution & migration strategy (FNV-1a → BLAKE3)

Lore currently uses 64-bit FNV-1a hex strings (`016x`, 16 hex characters) for fast inline source-artifact fingerprinting (`full_hash` and `prefix_hash`). When upgrading to BLAKE3 (64 hex characters) in a future migration:
1. **Additive Compatibility:** `full_hash` and `prefix_hash` columns are `TEXT` without fixed length constraints, requiring no breaking DDL.
2. **Dual-Hash Verification:** Skip detection and append validation compare hash length (16-char FNV-1a vs 64-char BLAKE3). Legacy FNV-1a hashes on unchanged files remain valid without triggering a mass re-ingestion storm; modified files automatically update to BLAKE3 on their next ingest.
3. **Graceful Upgrades:** Background worker passes can lazily upgrade idle records without taking exclusive database locks.

### ingest_state — committed parser checkpoint

`ingest_state(source_artifact_id PK, generation, last_offset, last_line, prefix_hash, parser_version, state, updated_at)`.

Durable `job` rows keep a bounded human-readable `error` plus a stable,
content-free `error_kind` (for example `source_io`, `sqlite_constraint`, or
`adapter_not_registered`) so local diagnostics never depend on parsing error text.

- Parser output after a checkpoint is buffered in a bounded batch.
- Cleartext is streamed through scanning and redacted projection construction **before** opening the write transaction; large blob temp files are content-addressed/finalized atomically, and unreferenced crash orphans are garbage-collected.
- Normalized upserts, count changes, search projections, and the new checkpoint commit in **one SQLite transaction**.
- On restart, resume only if generation/size/prefix hash prove the file is an append of the checkpointed bytes.
- On truncation, prefix mismatch, or in-place rewrite, increment generation and transactionally rebuild rows sourced from that artifact.
- Moving a rollout to `archived_sessions` adds a path alias; it does not create a duplicate session.

## 4. Conversation entities

### AgentSession

| field | type | notes |
|---|---|---|
| id | ULID PK | |
| agent_id | FK → Agent | |
| native_session_id | TEXT NULL | agent id when present |
| dedupe_key | TEXT | deterministic fallback when native id is absent |
| title | TEXT NULL | custom/AI/derived, with provenance in metadata |
| started_at / ended_at | INT | first/last normalized event |
| primary_model | TEXT NULL | list convenience only; per-turn model is canonical |
| message_count / tool_call_count | INT | denormalized, updated transactionally |
| token totals | INT NULL | input/output/cache totals where available |
| est_cost_usd | REAL NULL | derived with provider/model/price-version provenance |
| parse_status | TEXT | `ok | partial | failed` |
| parse_note | TEXT NULL | bounded diagnostic; no raw content/secrets; projected only on opened session details, not browse/search summaries |
| agent_version | TEXT NULL | parser fixture selector |
| metadata_json | JSON | versioned agent extras |

Constraints: unique `(agent_id, native_session_id)` when native id is non-null; unique `(agent_id, dedupe_key)` otherwise. `SessionSource(session_id, source_artifact_id, first_seq, last_seq)` supports copied/continued/archived sources without duplicating the logical session.

### SessionSegment — context valid for an event range

| field | type | notes |
|---|---|---|
| id | ULID PK | |
| session_id | FK → AgentSession | |
| seq_start / seq_end | INT | inclusive range; non-overlapping per session |
| cwd | TEXT NULL | recorded context for this range |
| repository_id | FK NULL | resolved with confidence |
| worktree_id | FK NULL | |
| model / provider | TEXT NULL | current turn context |
| context_source | TEXT | `event | session_header | inferred` |
| resolution_confidence | TEXT | `high | medium | low | unresolved` |

Create a new segment whenever recorded cwd/repository/worktree/model context changes. A session-level “primary repository” may be derived for list UI but is not canonical.

### Message — event/turn envelope

| field | type | notes |
|---|---|---|
| id | ULID PK | |
| session_id / segment_id | FK | |
| native_uuid | TEXT NULL | unique within session when present |
| parent_id | FK NULL | resolved message tree parent |
| parent_native_uuid | TEXT NULL | retained for out-of-order resolution |
| seq | INT | total order from source position |
| role | TEXT | `user | assistant | system | tool | meta` |
| event_kind | TEXT | `message | summary | compaction | attachment | title | mode | pr_link | other` |
| is_sidechain | BOOL | |
| ts | INT NULL | source timestamp |
| model | TEXT NULL | per-turn model |
| token fields | INT NULL | input/output/cache values |
| stop_reason | TEXT NULL | |
| source_artifact_id / source_offset | FK / INT | provenance back to source bytes |
| metadata_json | JSON | bounded, versioned extras |

### MessagePart — ordered mixed content

| field | type | notes |
|---|---|---|
| id | ULID PK | |
| message_id | FK → Message | |
| ordinal | INT | preserves block order; unique per message |
| kind | TEXT | `text | thinking | tool_use | tool_result | attachment | summary | opaque | other` |
| text | TEXT NULL | small canonical text |
| content_json | JSON NULL | structured block payload |
| blob_id | FK NULL | large text/binary/opaque payload |
| searchable | BOOL | false for opaque/encrypted and default false for thinking |
| metadata_json | JSON | e.g. thinking signature; never flatten away unknown fields |

An assistant turn with text → thinking → tool use remains one Message with three ordered MessageParts. A user `tool_result` block remains in its source Message and links to its ToolCall; it is not moved out of event order.

### ToolCall

| field | type | notes |
|---|---|---|
| id | ULID PK | |
| session_id | FK → AgentSession | |
| call_part_id | FK → MessagePart | source invocation block |
| result_part_id | FK NULL → MessagePart | source result block |
| native_call_id | TEXT | unique within session |
| name | TEXT | |
| input_json | JSON NULL | small input |
| input_blob_id | FK NULL | large input |
| output_text | TEXT NULL | small result projection |
| output_blob_id | FK NULL | large result |
| is_error | BOOL NULL | |
| duration_ms | INT NULL | |

Unique `(session_id, native_call_id)`. Calls and results may arrive out of order; unresolved links are finalized at transaction boundaries without losing either part.

### FileEvent

| field | type | notes |
|---|---|---|
| id | ULID PK | |
| session_id / segment_id | FK | |
| tool_call_id | FK NULL | |
| path | TEXT | sanitized, repo-relative when resolvable |
| change_kind | TEXT | `edit | write | create | delete | move | read | patch` |
| old_path | TEXT NULL | move/rename source |
| lines_added / lines_removed | INT NULL | **derived** from a parsed patch when available |
| patch_blob_id | FK NULL | byte-faithful recorded patch/diff content |
| source | TEXT | `agent_patch | agent_tool_input | lore_capture` |
| event_ts / observed_at | INT NULL | provenance time |

## 5. Repository and Git entities

### Repository

| field | type | notes |
|---|---|---|
| id | ULID PK | |
| identity_key | TEXT UNIQUE | opaque Lore identity; not a root SHA/path |
| display_name | TEXT | user-overridable |
| primary_path | TEXT NULL | latest hint |
| identity_confidence | TEXT | `confirmed | high | medium | low` |
| is_missing | BOOL | |
| created_at / updated_at | INT | |

### RepositoryIdentityEvidence

| field | type | notes |
|---|---|---|
| id | ULID PK | |
| repository_id | FK → Repository | |
| kind | TEXT | `git_common_dir | remote_root | remote | root_set | path | user_merge | user_split` |
| value_hash / display_value | TEXT | credentials stripped; sensitive paths may be display-redacted |
| confidence | TEXT | |
| first_seen_at / last_seen_at | INT | |

Root commits are stored as a sorted set because histories can have multiple roots. Root-only evidence does not auto-merge repositories.

### Worktree

| field | type | notes |
|---|---|---|
| id | ULID PK | |
| repository_id | FK → Repository | |
| path | TEXT | current path |
| git_common_dir_hash | TEXT | local grouping evidence; raw path remains app-private |
| branch_hint | TEXT NULL | last observed |
| is_primary / is_missing | BOOL | |

### GitObservation

| field | type | notes |
|---|---|---|
| id | ULID PK | |
| session_id / segment_id | FK | |
| source | TEXT | `agent_recorded | agent_patch | lore_captured | lore_reverified` |
| event_ts | INT NULL | time claimed by source event |
| observed_at | INT | when Lore read/verified it |
| temporal_confidence | TEXT | `exact_event | near_event | retrospective | current_only` |
| branch / commit_sha / commit_subject | TEXT NULL | |
| remote_url_norm | TEXT NULL | credentials removed |
| is_dirty / ahead / behind | INT NULL | only when observed/recorded |
| changed_files_json | JSON NULL | status summary, not an implied full diff |
| diff_blob_id | FK NULL | only for exact recorded or explicitly captured diff |
| commit_exists | BOOL NULL | verification result |
| metadata_json | JSON | source-specific evidence |

Multiple observations per segment are expected and never overwrite one another.

## 6. Blob, secret, and search entities

### Blob

`Blob(id PK, content_hash UNIQUE, media_type, byte_len, storage_relpath, compression, scan_state, hash_algo, created_at)` where `scan_state` is `pending | clean | findings | opaque_excluded | failed_quarantined` and `hash_algo` is `blake3 | fnv1a`.

- `content_hash` is the byte length followed by a **BLAKE3** digest, and it must stay cryptographic: the address is both the dedupe key (staging skips the write when the path exists) and the key the row's `scan_state` hangs off, so two payloads sharing an address means the second inherits the first's bytes *and* its completed secret scan. Migration 0010 introduced `hash_algo`; pre-0010 rows keep their `fnv1a` address (reads resolve `storage_relpath`, so they stay readable) and are re-addressed lazily when their source artifact is next re-ingested.
- Cleartext blobs must be fully streamed through secret scanning before becoming searchable/exportable.
- Opaque encrypted agent content uses `opaque_excluded` and is never decoded, searched, or exported.
- A scan failure quarantines the blob from search/export; there is no head+tail shortcut.

### SecretFinding

`SecretFinding(id PK, session_id, source_kind, source_id, field, rule, span_start, span_end, severity, value_fingerprint, disposition, created_at)`.

`source_kind/source_id/field` can target MessagePart text/JSON, ToolCall input/output, FileEvent patch, or a cleartext Blob. The canonical archive may still contain the raw value for faithful local viewing; findings prevent amplification into search, logs, and default exports. Lore never stores the value a second time in the finding or allowlist.

### SearchDocument and FTS5

`SearchDocument(id INTEGER PK, session_id, segment_id, source_kind, source_id, field, ordinal, redacted_text, created_at)` is the sole external-content table for `search_fts`.

- One row represents one searchable projection (message part, selected tool input, tool output, or patch).
- `redacted_text` never contains raw flagged spans.
- `search_fts` uses external content from **SearchDocument only**; it does not pretend Message and ToolCall are one content table.
- FTS5 `snippet()` markers produce highlighted excerpts. SQLite FTS5 `offsets()` is not part of the design; source navigation uses `source_kind/source_id/field` plus deterministic projection mappings.
- SearchDocument and FTS rows commit in the same transaction as their canonical source.

## 7. Skills (V0.5+)

`Skill(id, title, body_markdown, repository_id NULL, status, provenance_json, created_at, updated_at)` plus `SkillSource(skill_id, source_kind, source_id, evidence_note)`.

Every promoted claim links to concrete MessagePart/ToolCall/FileEvent/GitObservation evidence. The skill-promotion privacy model remains OPEN.

## 8. Performance-critical constraints and indexes

- `AgentSession(agent_id, started_at DESC)`; unique native/dedupe constraints described above.
- `AgentSession(started_at DESC, id DESC)` — the default browse order (`list_sessions`, `list_sessions_page`). The `agent_id`-leading index above cannot serve it, so without this every page cost a full scan plus a temp B-tree sort.
- `SessionSegment(session_id, seq_start)` and `(repository_id, session_id)`.
- `Message(session_id, seq)` unique; `Message(session_id, native_uuid)` partial unique; `Message(parent_id)`.
- `MessagePart(message_id, ordinal)` unique.
- `ToolCall(session_id, native_call_id)` unique; `FileEvent(session_id, path)`.
- `GitObservation(session_id, source, observed_at)`; `GitObservation(commit_sha)`.
- `RepositoryIdentityEvidence(kind, value_hash)`; `Worktree(repository_id, path)`.
- `SourceArtifact(agent_id, current_path)`; `SourceArtifact(agent_id, native_file_id, full_hash)` — ingest dedupe probes (`ingest_file`) must stay O(log n), not a growing full-table scan.
- `SearchDocument(session_id, source_kind, source_id)` and the FTS rowid relationship.
- `SecretFinding(source_kind, source_id, field)` for target lookup; `SecretFinding(session_id)` for the per-session count read on every session open (`query::secret_count`).
- `SessionSource(source_artifact_id)` — the primary key leads with `session_id`, so the `ON DELETE CASCADE` from `SourceArtifact` needs this to avoid a scan.
- `SessionFolder(folder_id, session_id)` for indexed folder filtering and thread count aggregation; `Folder(position, name)`.

Foreign keys are enabled. Adapter-derived children use explicit upserts or source-generation replacement; denormalized counts update in the same transaction. Keyset pagination uses stable `(started_at,id)` or `(seq,id)` cursors.

## 9. Storage layout and deletion boundary

```text
<app-data>/Lore/
  lore.db
  lore.db-wal / lore.db-shm
  blobs/
  backups/              local rolling SQLite backups; same permissions
  quarantine/           preserved corrupt-archive artifacts (never auto-deleted)
  logs/                 rotating, content-free/redaction-aware
  cache/                disposable rendered/search cache
  exports/              only exports explicitly kept inside app data
```

The archive is **SQLite plus its app-owned sidecars/directories**, not literally one file. External user-chosen exports and original agent logs are outside Lore's ownership and are never silently deleted. See `SECURITY.md` for permissions, backup limitations, and “Forget everything.”

## 10. M0/M1 acceptance criteria

- Schema migrations create all V0 entities and enforce the uniqueness/foreign-key rules above.
- A mixed Claude content array round-trips in order, including thinking metadata and tool result placement.
- A session that changes cwd produces separate SessionSegments.
- Append, truncate, rewrite, archive-move, duplicate-id, and out-of-order-parent fixtures are idempotent after restart.
- A large cleartext blob cannot enter FTS/export until a complete scan succeeds.
- Rebuilding SearchDocument/FTS never requires reparsing agent logs and never indexes raw flagged secrets.

## 11. Open modeling questions (OPEN)

- Thinking/reasoning remains viewable but not searchable by default; confirm whether opt-in indexing is desirable.
- Cross-source dedupe between SpecStory Markdown and native sessions needs evidence beyond fuzzy text; `SessionSource` supports linking once a safe key is defined.
- **DECIDED (M7):** Local-backup cadence and retention are user-configurable via `Setting` keys (`backup.interval` off/daily/weekly and `backup.keep` 1..100, default 7); recovery falls back across intact backups and does not assume original agent logs exist.
