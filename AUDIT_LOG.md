# Lore Security & Performance Audit Log

Durable audit memory for autonomous hardening and optimization passes.

---

## Shipped Items

### Item 1: `save_session_export` Arbitrary File Write Hardening
- **Claim**: `save_session_export` in `src-tauri/src/lib.rs` accepted arbitrary destination paths from the webview and wrote directly to disk with app permissions.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `save_session_export` previously took `path: String` with only basic length/control character checks. If invoked maliciously, it could overwrite arbitrary files (including Lore's database or agent configurations).
- **Fix**:
  - Implemented `validate_export_path` in `src-tauri/src/lib.rs`.
  - Enforced `path.is_absolute()`.
  - Rejected paths inside `state.archive_dir` (preventing database/blob corruption).
  - Rejected paths inside agent discovery roots (`~/.claude`, `~/.codex`, custom roots).
  - Delegated native file picking directly to Rust via `app.dialog()` (`blocking_save_file()`).
- **Files Touched**: `src-tauri/src/lib.rs`, `src/ipc.ts`, `src/App.tsx`, `docs/architecture/SECURITY.md`.
- **Checks Run**: `cargo test --workspace`, `npm run check`.

---

### Item 2: `ingest_file` 64 MiB Source File Size Cap
- **Claim**: `ingest_file` called `fs::read_to_string` unconditionally, allowing multi-GB files to cause worker process OOM.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `crates/lore-core/src/ingest.rs` read entire source files into a `String` before parsing.
- **Fix**:
  - Added `pub const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;` (64 MiB).
  - Oversized files stream only the initial `PREFIX_BYTES` (4096 bytes) for fingerprinting and metadata snapshotting.
  - Per the tolerant parsing rule, oversized sources degrade gracefully to `ParseStatus::Partial` with note `"source file exceeds maximum supported size (64MB)"`.
  - Added regression test `oversized_source_degrades_gracefully_to_partial` in `crates/lore-core/tests/ingest_file.rs`.
- **Files Touched**: `crates/lore-core/src/ingest.rs`, `crates/lore-core/tests/ingest_file.rs`, `docs/architecture/AGENT_ADAPTERS.md`.
- **Checks Run**: `cargo test -p lore-core --test ingest_file`.

---

### Item 3: Hardened Git Fallback Filter Defense & Absolute Path Enforcement
- **Claim**: Fallback system `git` executing `status --porcelain` or `diff` could trigger repo-local `.gitattributes` clean/textconv filter helper scripts under hostile clones.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `GIT_CONFIG_NOSYSTEM` and `GIT_CONFIG_GLOBAL=/dev/null` do not suppress repository-local `.git/config` and `.gitattributes` filter drivers executed by `git status` or `git diff`.
- **Fix**:
  - Removed `"status"` and `"diff"` from `ALLOWED_SUBCOMMANDS` in `crates/lore-core/src/git.rs`.
  - In `capture_via_git`, dirtiness is left as `None`; dirtiness evaluation is exclusive to pure-Rust `gix` (which does not execute shell filter commands).
  - Enforced `path.is_absolute()` in `capture` and `reverify`.
  - Added regression test `capture_declines_relative_paths` in `crates/lore-core/tests/git_capture.rs`.
- **Files Touched**: `crates/lore-core/src/git.rs`, `crates/lore-core/tests/git_capture.rs`, `docs/architecture/GIT_INTEGRATION.md`.
- **Checks Run**: `cargo test -p lore-core --test git_capture`.

---

### Item 4: Keyset Pagination for Session Messages
- **Claim**: `get_session` fetched all messages, parts, and file events across IPC in a single unbounded DTO, causing latency and webview pressure for large sessions.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `query::get_session` joined all `message_part` and `message` rows for a session without limits.
- **Fix**:
  - Added `MessagePage` DTO and `next_message_cursor` field to `SessionDetail` in `crates/lore-ipc/src/lib.rs`.
  - Added `pub fn list_session_messages_page` with keyset cursor (`WHERE session_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3`) in `crates/lore-core/src/query.rs`.
  - `get_session` now bounds initial messages to 200.
  - Exposed `list_session_messages_page` command in `src-tauri/src/lib.rs`.
  - Updated React `SessionView` to lazily stream subsequent pages via "Show more messages".
- **Files Touched**: `crates/lore-core/src/query.rs`, `crates/lore-ipc/src/lib.rs`, `crates/lore-ipc/bindings/SessionDetail.ts`, `crates/lore-ipc/bindings/MessagePage.ts`, `src-tauri/src/lib.rs`, `src/ipc.ts`, `src/components/SessionView.tsx`, `src/components/SessionView.test.tsx`, `docs/architecture/ARCHITECTURE.md`.
- **Checks Run**: `cargo test --workspace`, `npm run check`.

---

### Item 5: Stored Secret Findings at Export Time
- **Claim**: `export_session_markdown` re-scanned all message texts using live regexes at export time rather than reading stored findings.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `render_field` in `crates/lore-core/src/export.rs` called `secrets::scan(&text)` on every field during export.
- **Fix**:
  - `export_session_markdown` now queries stored `secret_finding` rows from SQLite for the session and masks spans via `secrets::redact(text, findings)`.
  - Added test `export_uses_stored_findings_independent_of_live_scanner` verifying exports mask secrets using stored database findings even if the live scanner is disabled.
- **Files Touched**: `crates/lore-core/src/export.rs`.
- **Checks Run**: `cargo test -p lore-core -- export`.

---

### Item 6: Zero-Allocation Hot Loops in Secret Keyword Scanning
- **Claim**: `near_secret_keyword` and `preceded_by_data_uri` allocated a `Vec<u8>` copy on every candidate match to perform case conversions.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `to_lower` vector allocation inside the search window loop in `crates/lore-core/src/secrets.rs`.
- **Fix**:
  - Replaced vector allocations with in-place ASCII-case-insensitive byte matching (`contains_ignore_ascii_case_bytes`).
- **Files Touched**: `crates/lore-core/src/secrets.rs`.
- **Checks Run**: `cargo test -p lore-core -- secrets`.

---

### Item 7: Dual-Hash Verification Migration Strategy (FNV-1a → BLAKE3)
- **Claim**: `source_artifact` `full_hash` and `prefix_hash` use FNV-1a. Switching hashes naively would invalidate all existing entries and trigger a full-archive re-parse.
- **Status**: DEFERRED-NEEDS-HUMAN (Architecture design documented)
- **Evidence / Strategy**:
  - Documented dual-hash verification migration design in `docs/architecture/DATA_MODEL.md`.
  - 16-char (FNV-1a) vs 64-char (BLAKE3) length differentiation allows existing archives to remain valid without forced re-parsing; modified files upgrade to BLAKE3 on subsequent ingests.
- **Files Touched**: `docs/architecture/DATA_MODEL.md`.

---

### Item 8: Rollback Journal Sidecar Quarantine in Recovery
- **Claim**: `quarantine` in `recovery.rs` preserved `-wal` and `-shm` sidecars but omitted `-journal` (rollback journal mode).
- **Status**: CONFIRMED & FIXED
- **Evidence**: Loop in `recovery.rs` line 144 was `for suffix in ["", "-wal", "-shm"]`.
- **Fix**:
  - Added `"-journal"` suffix to the quarantine loop.
- **Files Touched**: `crates/lore-core/src/recovery.rs`.
- **Checks Run**: `cargo test -p lore-core --test recovery`.

---

### Item 9: Poisoned Job Crash Recovery Protection
- **Claim**: Unchecked crash recovery in `jobs::recover_running` unconditionally returned all running jobs to `pending`, creating an infinite restart crash loop if a source file caused an unhandled crash or abort.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `recover_running` previously had no attempt threshold and reset any running job to pending.
- **Fix**:
  - Added `MAX_JOB_ATTEMPTS = 5`.
  - `recover_running` automatically marks jobs with `attempts >= MAX_JOB_ATTEMPTS` as `failed` with `error_kind: 'poisoned'` and diagnostic `"job exceeded maximum crash recovery attempts"`.
  - `schedule_source` resets `attempts = 0` when re-arming a job for an updated source payload.
  - Added regression test `recover_running_fails_poisoned_jobs_exceeding_max_attempts` in `crates/lore-core/src/jobs.rs`.
- **Files Touched**: `crates/lore-core/src/jobs.rs`.
- **Checks Run**: `cargo test -p lore-core -- jobs`.

---

### Item 10: React UI ErrorBoundary & Render Exception Containment
- **Claim**: Any rendering exception (e.g. malformed markdown, unhandled part structure, or edge-case JSON) unmounted the entire React component tree, blanking the application window without recovery.
- **Status**: CONFIRMED & FIXED
- **Evidence**: No React error boundaries existed in `src/`.
- **Fix**:
  - Created `src/components/ErrorBoundary.tsx` providing safe local rendering error capture, a user-friendly alert, and "Try again" / "Reload window" recovery actions.
  - Wrapped root `App` and `SessionView` in `ErrorBoundary`.
  - Added unit test suite `src/components/ErrorBoundary.test.tsx`.
- **Files Touched**: `src/components/ErrorBoundary.tsx`, `src/components/ErrorBoundary.test.tsx`, `src/main.tsx`, `src/App.tsx`, `src/styles.css`.
- **Checks Run**: `npm run check`.

### Item 11: Adapter Tolerant Parsing for Alternative Tool Paths and String Arrays
- **Claim**: Claude `NotebookEdit` events with `notebook_path` or `path` input keys were dropped as file events, and string array elements in Codex `content`/`summary` and Claude tool results were not parsed.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `file_event_from_tool` checked only `"file_path"`; `text_parts` and `push_reasoning_text` only checked `{ text: "..." }` objects.
- **Fix**:
  - `file_event_from_tool` checks `file_path`, `notebook_path`, and `path`.
  - `tool_result_text`, `text_parts`, and `push_reasoning_text` parse both plain strings and `{ text: "..." }` object blocks.
  - Added unit tests `extracts_notebook_edit_file_events` in `claude_code.rs` and `parses_string_arrays_in_message_content_and_reasoning` in `codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`, `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters`.

---

### Item 12: DiffBlock and CommandPalette Bounding
- **Claim**: Inline patch rendering in `SessionView` could render unbounded numbers of DOM elements for giant diffs, and `CommandPalette` fuzzy score took unbounded query lengths.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `DiffBlock` previously split and rendered all lines of `text` regardless of length; `fuzzyScore` operated on unbounded strings.
- **Fix**:
  - Added `MAX_DIFF_LINES = 1_000` cap in `DiffBlock` with an explicit `… truncated (N more lines)` indicator.
  - Bounded `fuzzyScore` and command palette `<input>` `maxLength` to 256 characters.
  - Added unit test `bounds_oversized_diffs_to_1000_lines_with_a_truncation_note` in `SessionView.test.tsx`.
- **Files Touched**: `src/components/DiffBlock.tsx`, `src/components/SessionView.tsx`, `src/components/CommandPalette.tsx`, `src/components/SessionView.test.tsx`.
- **Checks Run**: `npm run check`.

### Item 13: Partial Backup Cleanup on Online Copy Failure
- **Claim**: If SQLite online backup copy errored midway, the incomplete database file was left on disk and could be listed as a valid backup.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `create_backup` in `crates/lore-core/src/backup.rs` only unlinked the target file on `set_private` or `verify` errors, not if `backup.run_to_completion(...)` failed.
- **Fix**: Wrapped online copy execution in a result block and unlinked the partial file if `run_to_completion` returns `Err`.
- **Files Touched**: `crates/lore-core/src/backup.rs`.
- **Checks Run**: `cargo test -p lore-core --test backup`.

### Item 14: Obsolete Webview Save Dialog Wrapper Cleanup
- **Claim**: `src/ipc.ts` still exported a `chooseExportFilePath` helper relying on webview-side dialogs, which was superseded when native save dialogs were moved to Rust in `save_session_export`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `save_session_export` in `src-tauri/src/lib.rs` directly invokes `app.dialog().file().blocking_save_file()`, leaving `chooseExportFilePath` unused in the frontend.
- **Fix**: Removed `chooseExportFilePath` and unused `@tauri-apps/plugin-dialog` `save` import from `src/ipc.ts`.
- **Files Touched**: `src/ipc.ts`.
- **Checks Run**: `npm run check`.

### Item 15: Bounded Folder Creation and Renaming Inputs
- **Claim**: Folder name input fields in `FolderList` did not set `maxLength`, allowing unbounded typed characters before hitting the 100-char backend truncation limit.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `FolderList.tsx` previously declared `<input>` elements without `maxLength`.
- **Fix**: Added `maxLength={100}` on both the new folder input and rename input in `FolderList.tsx`.
- **Files Touched**: `src/components/FolderList.tsx`.
- **Checks Run**: `npm run check`.

### Item 16: Structured Tool Result Text Extraction in Claude Code
- **Claim**: In Claude Code transcripts, `tool_result` message parts with array content (e.g. `[{"type":"text","text":"..."}]` or `["line 1", "line 2"]`) yielded `text: None` on the message part, leaving them unindexed by search.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `parts_from_content` in `claude_code.rs` only used `block.get("content").and_then(Value::as_str)` for `"tool_result"`, ignoring structured arrays.
- **Fix**: Updated `parts_from_content` to use `tool_result_text(block)` for `"tool_result"` and added handling for plain string elements in content arrays. Added assertions to `extracts_notebook_edit_file_events` test.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters`.

### Item 17: Fuzz Parser Invariant Strengthening
- **Claim**: `assert_consistent` in `fuzz_parse.rs` checked segment bounds and tool coordinates, but did not assert strict monotonic ordering of message sequence numbers or segment span validity.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `assert_consistent` in `tests/fuzz_parse.rs` previously did not check `message.seq > prev` or `segment.seq_start <= segment.seq_end`.
- **Fix**: Added assertions in `assert_consistent` enforcing `message.seq >= 0`, strictly monotonic `message.seq > prev`, non-negative part ordinals, and valid `segment.seq_start <= segment.seq_end` across 4,000 randomized hostile documents.
- **Files Touched**: `crates/lore-core/tests/fuzz_parse.rs`.
- **Checks Run**: `cargo test -p lore-core --test fuzz_parse`.

### Item 18: Timezone Offset Coverage in Common Adapter Helpers
- **Claim**: `epoch_ms` in `adapters::common` was only tested against UTC timestamps ending in `Z`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `epoch_ms_parses_rfc3339_with_optional_whitespace` in `common.rs` lacked tests with explicit `+` / `-` timezone offsets.
- **Fix**: Added assertions verifying positive (`+02:00`) and negative (`-05:00`) timezone offset parsing.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

### Item 19: Codex Start Telemetry Event Recognition
- **Claim**: Codex telemetry events `mcp_tool_call_start` and `web_search_start` were not listed in known telemetry event subtypes, causing them to be flagged as unknown events.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `event_msg` match pattern in `codex.rs` previously included `mcp_tool_call_end` and `web_search_end`, but omitted the corresponding start events.
- **Fix**: Added `mcp_tool_call_start` and `web_search_start` to ignored telemetry event arms in `codex.rs` and added assertions in `unknown_event_msg_is_flagged_but_known_telemetry_is_not`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

### Item 20: Search Query & Keyset Cursor Fuzz Coverage
- **Claim**: `fuzz_parse.rs` fuzzed adapter parsers and secret scanners, but did not exercise `search_page` query tokenization or keyset cursor decoding against hostile input.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `search_page` query parsing previously lacked randomized adversarial coverage in the integration fuzz suite.
- **Fix**: Added `search_and_cursor_fuzz_never_panic_on_adversarial_queries` to `fuzz_parse.rs`, running 2,000 randomized iterations testing hostile control chars, zero-width spaces, special FTS5 operators (`NEAR`, `NOT`, unclosed quotes), and corrupted cursors across `Relevance`, `Newest`, and `Oldest` sort modes.
- **Files Touched**: `crates/lore-core/tests/fuzz_parse.rs`.
- **Checks Run**: `cargo test -p lore-core --test fuzz_parse`.

---

### Item 21: SessionList Dynamic Navigation & Home Key Coverage
- **Claim**: `SessionList` keyboard navigation tests covered Arrow keys and End key, but lacked coverage for Home key navigation and listKey index reset on dynamic sessions list prop updates.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `src/components/SessionList.test.tsx` previously did not test `Home` key navigation or `listKey` resetting when `sessions` array updates.
- **Fix**: Added unit test in `SessionList.test.tsx` verifying `Home` key navigation and index reset to 0 on session list change.
- **Files Touched**: `src/components/SessionList.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 22: Multi-Segment Same Repository Resolution Coverage
- **Claim**: `enrich_session` integration tests covered single-segment sessions and worktrees, but lacked tests for multi-segment sessions spanning multiple subdirectories of the same repository.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `tests/enrich.rs` previously did not test multi-segment sessions in subdirectories of a single repository.
- **Fix**: Added `multi_segment_session_in_same_repo_resolves_to_single_repository` in `tests/enrich.rs`, asserting that multi-segment sessions across subdirectories link all segments to the exact same repository row.
- **Files Touched**: `crates/lore-core/tests/enrich.rs`.
- **Checks Run**: `cargo test -p lore-core --test enrich`.

---

### Item 23: CommandPalette Focus Restoration on Dismissal
- **Claim**: `CommandPalette` tests covered fuzzy scoring, keyboard navigation, and backdrop dismiss, but did not assert focus restoration to the active trigger element when the palette unmounts.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `src/components/CommandPalette.test.tsx` lacked an unmount focus assertion.
- **Fix**: Added unit test in `CommandPalette.test.tsx` verifying that unmounting `CommandPalette` restores focus to the previously active element.
- **Files Touched**: `src/components/CommandPalette.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 24: Claude Code Empty & Null Content Block Resilience
- **Claim**: `claude_code` adapter tests covered complex tool use and text messages, but lacked explicit assertions for empty string, empty array, and null `message.content` payloads.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` unit tests lacked explicit null/empty content test cases.
- **Fix**: Added `parses_empty_and_null_content_blocks_cleanly` in `claude_code.rs`, confirming that empty string, empty array, and null content blocks parse cleanly to `Ok` status with correct part counts.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 25: Multi-Remote Credential Normalization & Git Capture Coverage
- **Claim**: `git::capture` tests covered single remotes, but lacked test coverage verifying that multiple remotes with embedded credentials (e.g. `https://token@...` and `git@...`) are all properly stripped, normalized, sorted, and deduplicated.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `tests/git_capture.rs` lacked multi-remote credential normalization test cases.
- **Fix**: Added `capture_handles_multiple_remotes_and_commits` in `tests/git_capture.rs`, asserting that multiple remote URLs with credentials are normalized, stripped of sensitive tokens, sorted, and deduplicated in `CapturedRepo.remotes`.
- **Files Touched**: `crates/lore-core/tests/git_capture.rs`.
- **Checks Run**: `cargo test -p lore-core --test git_capture`.

---

### Item 26: Unified Diff Line Counting Multi-Hunk and CRLF Coverage
- **Claim**: `unified_diff_line_counts` tests in `common.rs` only covered a basic 1-addition 1-deletion LF diff without exercising multiple hunks, CRLF line endings, trailing `\ No newline at end of file` lines, or pure-addition diffs.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `adapters::common::tests::diff_counts_ignore_headers` lacked multi-hunk and CRLF test cases.
- **Fix**: Added test assertions in `common.rs` verifying accurate addition/deletion counts across CRLF line endings, multiple hunks, trailing no-newline markers, and creation diffs.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 27: Codex Null & Empty Reasoning and Content Tolerance
- **Claim**: `codex` adapter tests covered multi-line string arrays in reasoning and message content, but lacked tests for null `summary`, empty `content` arrays, and null `message.content` payloads.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` unit tests lacked explicit null/empty reasoning and message content test cases.
- **Fix**: Added `parses_empty_or_null_reasoning_and_message_content` in `codex.rs`, asserting that empty strings, null message contents, and null/empty array reasoning payloads parse cleanly to `Ok` status with correct part counts.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 28: SearchResults Query Change Active Index Reset
- **Claim**: `SearchResults` listbox keyboard navigation tests covered arrow keys, j/k, Home/End, and `onExitUp`, but lacked assertions confirming that the roving active index resets to `0` when the `query` prop changes.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SearchResults.test.tsx` lacked test coverage for query prop updates resetting navigation.
- **Fix**: Added unit test in `SearchResults.test.tsx` verifying that updating `query` resets `aria-activedescendant` and active index to the first result (`search-result-0`).
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 29: Quarantine Sidecar File Sweep Coverage
- **Claim**: `recover_archive` tests verified `lore.db` preservation, but did not test that all SQLite sidecar files (`lore.db-wal`, `lore.db-shm`, and `lore.db-journal`) are moved into `quarantine/` without leaving orphaned journal files.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `tests/recovery.rs` lacked a multi-sidecar quarantine test case.
- **Fix**: Added `recover_archive_quarantines_all_sidecar_files_including_wal_shm_journal` in `tests/recovery.rs`, asserting that all sidecars (`-wal`, `-shm`, and `-journal`) are relocated into the timestamped quarantine directory and cleared from `archive_dir`.
- **Files Touched**: `crates/lore-core/tests/recovery.rs`.
- **Checks Run**: `cargo test -p lore-core --test recovery`.

---

### Item 30: Codex Unknown Response Item Degradation Coverage
- **Claim**: `codex` adapter tests checked unknown `event_msg` telemetry types, but did not test unknown `response_item` types (such as future/newer items).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests lacked a test specifically verifying that unknown `response_item` types degrade to partial while keeping earlier valid messages intact.
- **Fix**: Added `unknown_response_item_type_degrades_partial` in `codex.rs`, asserting that unknown `response_item` types note partial status without discarding prior valid messages.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 31: BackupSettings Retention Count Clamping & Persistence Coverage
- **Claim**: `BackupSettings` component tests covered interval selection and on-demand backup triggers, but lacked tests for custom numeric retention input changes and bounding (`clamp` between 1 and 100).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `src/components/BackupSettings.test.tsx` lacked test assertions for `Keep newest` input changes and boundary clamping.
- **Fix**: Added unit test in `BackupSettings.test.tsx` verifying that changing the retention count persists the number and clamps out-of-bounds values (e.g. 0 -> 1, 200 -> 100).
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 32: Claude Code Subagent Sidechain & Parent UUID Parsing Coverage
- **Claim**: `claude_code` adapter tests verified tool pairs and basic text messages, but lacked explicit assertions for subagent sidechains (`isSidechain = true`), native `uuid`, and `parentUuid` linkage.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` unit tests lacked explicit subagent parent UUID parsing test cases.
- **Fix**: Added `parses_sidechain_subagent_messages_with_parent_uuids` in `claude_code.rs`, asserting that sidechain messages retain `is_sidechain = true`, `native_uuid`, and point to their parent message's `parent_native_uuid`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 33: Markdown Header & Bullet Stripping in Fallback Title
- **Claim**: `fallback_title` tests covered basic single-line truncation, but lacked explicit assertions for stripping bullet markers (`- `, `* `) and multi-level markdown headers (`### `).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` unit tests lacked explicit assertions for bullet points and `###` headers.
- **Fix**: Added test assertions in `adapters::common::tests::fallback_title_is_single_line_and_bounded` verifying that bullet points (`- `) and nested headers (`### `) are properly stripped.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 34: SettingsPanel Close Button and Backdrop Click Dismissal
- **Claim**: `SettingsPanel` modal tests covered Escape key navigation and Tab focus trapping, but lacked assertions verifying that clicking the close icon button (`✕`) or the backdrop element (`.modal-backdrop`) properly calls `onClose`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SettingsPanel.test.tsx` lacked explicit test cases for close button and backdrop click handlers.
- **Fix**: Added unit test in `SettingsPanel.test.tsx` verifying that clicking the close button or clicking the modal backdrop invokes `onClose`.
- **Files Touched**: `src/components/SettingsPanel.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 35: Pruning Terminal Jobs Retention Logic
- **Claim**: The job queue stored terminal rows (`done` and `failed`) indefinitely without a bounded maintenance cleanup function.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `crates/lore-core/src/jobs.rs` lacked a function to prune old terminal jobs.
- **Fix**: Added `prune_terminal_jobs` in `jobs.rs` using a deterministic `ORDER BY updated_at DESC, rowid DESC LIMIT -1 OFFSET ?1` query to purge oldest terminal records beyond `keep`, while strictly preserving active (`pending` and `running`) jobs.
- **Files Touched**: `crates/lore-core/src/jobs.rs`.
- **Checks Run**: `cargo test -p lore-core -- jobs`.

---

### Item 36: Codex Sparse Token Count Capture Coverage
- **Claim**: `codex` adapter token parsing tests verified full `total_token_usage` and flat legacy objects, but lacked assertions for sparse objects (e.g. only cached input tokens).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests lacked a sparse `total_token_usage` payload test case.
- **Fix**: Added `token_count_sparse_or_partial_fields_are_captured` in `codex.rs`, asserting that partial token payloads containing only cache tokens parse cleanly to `Tokens { input: None, output: None, cache: Some(50) }`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 37: Codex Compaction Marker Parsing
- **Claim**: `codex` adapter tests must verify both top-level `compacted` (with custom summary message) and telemetry `event_msg: context_compacted` markers.
- **Status**: CONFIRMED & VERIFIED
- **Evidence**: Verified in `top_level_compacted_and_context_compacted_become_markers` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 38: Codex Tool Call and Output Missing `call_id` Degradation
- **Claim**: `codex` adapter tests covered missing matching call IDs (`ghost`), but lacked tests asserting that `function_call` and `function_call_output` records without any `call_id` key degrade to partial status without loss of linear messages.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests lacked test cases for records completely missing the `call_id` property.
- **Fix**: Added `tool_call_and_output_without_call_id_degrade_partial` in `codex.rs`, asserting that records missing `call_id` note partial status while retaining sequential messages.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 39: Discovery Resilience with Non-Existent Roots
- **Claim**: `discover` was tested with populated fixture paths and symlink structures, but lacked tests confirming that non-existent custom override roots do not panic and report `installed = false` with 0 candidate sessions.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `discovery.rs` tests lacked a test specifically providing non-existent paths in `DiscoveryConfig`.
- **Fix**: Added `discovery_tolerates_non_existent_custom_roots` in `discovery.rs`, asserting that non-existent paths across all adapters report `installed = false` and 0 sessions.
- **Files Touched**: `crates/lore-core/src/discovery.rs`.
- **Checks Run**: `cargo test -p lore-core -- discovery`.

---

### Item 40: JSON Field and String Field Typed Value Handling
- **Claim**: `str_field` and `json_field` unit tests covered simple strings and nested objects, but lacked assertions for booleans, array values, and explicit nulls.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` unit tests lacked explicit assertions for booleans, arrays, and null fields in `str_field` and `json_field`.
- **Fix**: Extended `json_and_str_field_extract_values` in `common.rs`, asserting that `str_field` safely ignores non-string types (returning `None`) while `json_field` serializes boolean, array, and null values as expected.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 41: Claude Code Tool Result Error Flag with Null Content
- **Claim**: `claude_code` adapter tests verified tool result error capture with text content, but lacked assertions verifying that error tools with `content: null` capture `is_error = Some(true)` and `output_text = None` without failure.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tests lacked a null-content error tool result test case.
- **Fix**: Extended `tool_result_error_flag_is_captured` in `claude_code.rs`, asserting that tool results with `is_error: true` and `content: null` properly record `is_error: Some(true)` and `output_text: None`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 42: Codex Model Provider Normalization
- **Claim**: `session_meta` records containing `model_provider` values (e.g. `anthropic`, `openai`) must propagate onto every derived segment.
- **Status**: CONFIRMED & VERIFIED
- **Evidence**: Verified in `non_openai_provider_is_preserved` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 43: Ingest File Size Cap and State Transitions
- **Claim**: Source files exceeding 64 MiB (`MAX_SOURCE_BYTES`) degrade cleanly to `partial` with a content-free note, while files within limits transition across `New`, `Appended`, `Rewritten`, `Truncated`, and `Skipped`.
- **Status**: CONFIRMED & VERIFIED
- **Evidence**: Verified in `oversized_source_degrades_gracefully_to_partial` and `append_rewrite_and_truncate_replace_rows_idempotently` in `crates/lore-core/tests/ingest_file.rs`.
- **Files Touched**: `crates/lore-core/tests/ingest_file.rs`.
- **Checks Run**: `cargo test -p lore-core --test ingest_file`.

---

### Item 44: Codex Reasoning Block with Null Summary and Content
- **Claim**: `codex` adapter tests covered null summary or null content individually, but lacked tests asserting that reasoning blocks with both `summary: null` and `content: null` parse without generating empty thinking parts.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests lacked a combined null-summary and null-content test case.
- **Fix**: Extended `parses_empty_or_null_reasoning_and_message_content` in `codex.rs`, asserting that reasoning blocks with both `summary: null` and `content: null` yield 0 parts and retain message order.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 45: Claude Code Thinking Block Signature Metadata Coverage
- **Claim**: `claude_code` adapter thinking block parsing extracts optional `signature` into `metadata_json` while enforcing `searchable = false` for privacy, but lacked a dedicated unit test covering both signature-present and signature-absent blocks.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` unit tests lacked explicit assertions for thinking block `metadata_json` extraction.
- **Fix**: Added `parses_thinking_content_blocks_with_and_without_signatures` in `claude_code.rs`, asserting that thinking blocks extract `{"signature":"..."}` when present, keep `metadata_json = None` when absent, and strictly set `searchable = false`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 46: SessionView Segment Count and Untitled Fallback Handling
- **Claim**: `SessionView` header tests verified title display and partial-status notices, but lacked assertions testing multi-segment count labels (`2 segments`) and fallback to `(untitled session)` when `summary.title` is null.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SessionView.test.tsx` lacked test cases for multi-segment headers and null titles.
- **Fix**: Added unit test in `SessionView.test.tsx` asserting that null title renders `(untitled session)` and multi-segment sessions render `2 segments`.
- **Files Touched**: `src/components/SessionView.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 47: Secret Scanner Multi-Token Detection and Redaction
- **Claim**: `secrets.rs` tests covered single tokens and adjacent tokens, but lacked assertions verifying multiple distinct rule tokens (`github-token` and `slack-token`) across separate lines in one pass.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `secrets.rs` tests lacked a multi-line multi-token test case.
- **Fix**: Added `scan_detects_multiple_distinct_tokens_in_one_pass` in `secrets.rs`, asserting that GitHub and Slack tokens across separate lines are both identified and cleanly masked with their respective rule tags.
- **Files Touched**: `crates/lore-core/src/secrets.rs`.
- **Checks Run**: `cargo test -p lore-core -- secrets`.

---

### Item 48: Recovery Blocked Quarantine Directory Error Handling
- **Claim**: `recover_archive` tests verified corrupted database files and backup fallbacks, but lacked assertions testing clean `RecoveryError::Io` return when quarantine directory creation fails (e.g. blocked by a file).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `tests/recovery.rs` lacked a test for blocked quarantine directory creation.
- **Fix**: Added `recover_archive_fails_cleanly_when_quarantine_dir_is_blocked` in `tests/recovery.rs`, asserting that filesystem IO barriers return `Err(RecoveryError::Io)` content-free.
- **Files Touched**: `crates/lore-core/tests/recovery.rs`.
- **Checks Run**: `cargo test -p lore-core --test recovery`.

---

### Item 49: Migration Schema Checksum Integrity Verification
- **Claim**: `schema.rs` verified tables, indexes, and foreign keys, but lacked tests confirming that `schema_migrations` records all 10 SQL migrations with valid FNV-1a checksums in ascending version order.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `tests/schema.rs` lacked a migration record count and checksum verification test case.
- **Fix**: Added `schema_migrations_records_all_applied_migrations_with_checksums` in `tests/schema.rs`, asserting that all 10 migrations are recorded with valid 16-hex checksums.
- **Files Touched**: `crates/lore-core/tests/schema.rs`.
- **Checks Run**: `cargo test -p lore-core --test schema`.

---

### Item 50: Path Sanitizer Redundant Separators and Traversal Resolution
- **Claim**: `sanitize_path` tests covered basic `../` traversal, but lacked assertions testing consecutive duplicate separators (`///`, `\\\\`), self-references (`././`), and multi-level parent directory reductions (`a/b/c/../../d.ts`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` unit tests lacked explicit assertions for multi-level and consecutive separator normalization.
- **Fix**: Extended `sanitize_strips_traversal` in `common.rs`, asserting that redundant slashes and nested parent references resolve cleanly to relative canonical forms.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 51: Unified Diff Line Counts Git Metadata Header Filtering
- **Claim**: `unified_diff_line_counts` must ignore git diff metadata headers (such as `diff --git`, `new file mode`, and `index`) without counting them as added/removed lines.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` diff tests covered `---`/`+++` and `@@` headers, but lacked git-specific header lines.
- **Fix**: Extended `diff_counts_ignore_headers` in `common.rs`, asserting that git diff headers are safely ignored while lines added/removed in hunks are accurately counted.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 52: Path Sanitizer Control Characters and Zero-Width Filtering
- **Claim**: `sanitize_path` must strip control characters (like `\x1b` ANSI escape sequences and `\0` null bytes) and Unicode zero-width characters across directory segments.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` unit tests tested `\0` and `\u{200b}`, but lacked coverage for BOM `\u{feff}`, zero-width joiners `\u{200d}`, and ANSI escape sequences.
- **Fix**: Extended `sanitize_strips_traversal` in `common.rs`, asserting that ANSI escape sequences and zero-width code points are stripped cleanly without corrupting the path.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 53: JSON Non-Negative Integer Parser Boundary and Overflow Handling
- **Claim**: `non_negative_int_field` must safely handle boundary conditions including `i64::MAX`, reject `u64::MAX` overflow above signed 64-bit limits, and ignore floats and booleans.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` unit tests tested negative integers and strings, but lacked coverage for `i64::MAX`, `u64::MAX` overflow, floats, and booleans.
- **Fix**: Extended `non_negative_int_field_validates_bounds` in `common.rs`, asserting that `i64::MAX` is parsed as `Some(i64::MAX)` while `u64::MAX`, floats, and booleans return `None`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 54: Fallback Title CJK Multibyte and Character Boundary Truncation
- **Claim**: `title_from_text` must parse CJK character lines with leading bullets/stars without corrupting multibyte UTF-8 boundaries upon truncating to `TITLE_MAX_CHARS`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` fallback title tests only used ASCII characters for truncation assertions.
- **Fix**: Extended `fallback_title_is_single_line_and_bounded` in `common.rs`, asserting that CJK multibyte characters and bullet markers format cleanly and truncate safely to `TITLE_MAX_CHARS + 1` characters ending with `…`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 55: RFC3339 Subsecond Precision Parsing
- **Claim**: `epoch_ms` must parse RFC3339 timestamp strings with millisecond and microsecond fractional second components accurately to epoch milliseconds.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` timestamp tests verified `.000Z` and timezone offsets, but lacked fractional second precision test assertions.
- **Fix**: Extended `epoch_ms_parses_rfc3339_with_optional_whitespace` in `common.rs`, asserting that fractional seconds (`.123Z` and `.123456Z`) map to the exact millisecond offset.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 56: Codex Patch Apply End Null and Empty Content Handling
- **Claim**: `patch_apply_end` file change records containing `content: ""` must set `patch_text: Some("")`, and `content: null` must set `patch_text: None` without panicking or failing the parse.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests covered non-empty patch contents and deletes, but lacked assertions for empty string `""` and `null` content blocks.
- **Fix**: Added `parses_patch_apply_end_with_null_and_empty_content_and_empty_changes` in `codex.rs`, asserting that `content: ""` and `content: null` are parsed cleanly into `FileChangeKind::Create` file events.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 58: Codex Patch Apply End Empty Changes Object
- **Claim**: `patch_apply_end` records whose `changes` payload is an empty JSON object (`{}`) must not fail or flag the session as partial.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests covered malformed non-object `changes`, but lacked assertions for empty `{}` objects.
- **Fix**: Added `parses_patch_apply_end_with_null_and_empty_content_and_empty_changes` in `codex.rs`, asserting that `{}` yields 0 file events and status `Ok`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 57: Search Query Phrase Quoting for Boolean Keywords and Parentheses
- **Claim**: `fts_match` must convert boolean keywords (`AND`, `OR`) and tokens with parentheses into quoted phrase terms, preventing query operator injection in FTS5.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `search.rs` query tests covered basic string quoting and null bytes, but lacked assertions for parentheses and SQL/FTS boolean keywords.
- **Fix**: Extended `fts_match_quotes_terms_and_neutralizes_operators` in `search.rs`, asserting that boolean operators and parentheses become safely isolated double-quoted phrases.
- **Files Touched**: `crates/lore-core/src/search.rs`.
- **Checks Run**: `cargo test -p lore-core -- search`, `cargo clippy --all-targets -- -D warnings`.

---

### Item 61: FTS Search Query Zero-Width Character and Null Byte Discard
- **Claim**: `fts_match` must return `None` when given terms composed entirely of null bytes or Unicode zero-width characters (`\u{200b}`, `\u{feff}`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `search.rs` tests covered `\0`, but lacked explicit assertions for multi-codepoint zero-width tokens returning `None`.
- **Fix**: Extended `fts_match_quotes_terms_and_neutralizes_operators` in `search.rs`, asserting that terms composed of only zero-width code points discard cleanly and produce `None`.
- **Files Touched**: `crates/lore-core/src/search.rs`.
- **Checks Run**: `cargo test -p lore-core -- search`.

---

### Item 59: Codex Message Role Mapping and Fallbacks
- **Claim**: Codex message items must map `"assistant"`, `"system"`, `"tool"`, and default unknown roles to `Role::User` without failing the parse.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests covered `Role::User` and `Role::Assistant`, but lacked explicit assertions for `Role::System` and unknown role fallbacks.
- **Fix**: Added `parses_message_with_system_and_custom_roles_and_mixed_content_array` in `codex.rs`, asserting that `role: "system"` maps to `Role::System` and unrecognized roles map to `Role::User`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 60: Codex Mixed Content Array and String Array Payloads
- **Claim**: Codex message items whose `content` is an array containing both raw strings and structured objects with a `text` key (`["first", {"text": "second"}]`) must preserve all parts in order with correct ordinals.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests covered string arrays and object arrays separately, but lacked mixed-array assertions.
- **Fix**: Added `parses_message_with_system_and_custom_roles_and_mixed_content_array` in `codex.rs`, asserting that mixed arrays preserve sequential parts with consecutive ordinals.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 62: Codex Response Item Message Sequential Parsing
- **Claim**: Multiple consecutive `response_item` messages in a Codex session must maintain monotonic sequence numbers (`seq`), preserve timestamps, and correctly map into `ParsedMessage` records.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_message_with_system_and_custom_roles_and_mixed_content_array` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 63: Multi-Segment Session Re-Ingest Idempotency
- **Claim**: Re-ingesting an untouched source file containing a multi-segment session must return `IngestOutcome::Skipped`, preserving exact message and segment counts in SQLite.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `tests/ingest_file.rs` covered single-segment appends and rewrites, but lacked assertions for untouched multi-segment re-ingests.
- **Fix**: Added `identical_reingest_of_multi_segment_session_is_a_noop` in `tests/ingest_file.rs`, asserting that untouched files return `Skipped` and maintain identical database state.
- **Files Touched**: `crates/lore-core/tests/ingest_file.rs`.
- **Checks Run**: `cargo test -p lore-core --test ingest_file`.

---

### Item 64: Claude Code Tool Use Input Serialization and Missing ID Degradation
- **Claim**: `tool_use` blocks with primitive values (string, number, null) must serialize properly into `input_json`, and tool use records without an `id` must degrade to `partial` with a diagnostic without dropping preceding valid tool calls.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` unit tests tested object inputs, but lacked assertions for primitive input serialization and missing id degradation.
- **Fix**: Added `parses_tool_use_with_non_object_and_primitive_inputs` in `claude_code.rs`, asserting that primitive inputs serialize cleanly to `input_json` and missing ids note partial status.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 65: Epoch Timestamp Calendar and Boundary Validation
- **Claim**: `epoch_ms` must safely reject out-of-bounds calendar components (month > 12, hour > 23, invalid leap dates) returning `None`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` timestamp tests verified RFC3339 formats, but lacked negative calendar assertions.
- **Fix**: Extended `epoch_ms_parses_rfc3339_with_optional_whitespace` in `common.rs`, asserting that invalid calendar months, hours, and non-leap Feb 29 dates return `None`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 68: Leap Year Timestamp Calculation in RFC3339 Parser
- **Claim**: `epoch_ms` must parse valid leap year dates (`2024-02-29T12:00:00Z`) successfully to positive epoch milliseconds.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` timestamp tests lacked explicit leap year validation.
- **Fix**: Extended `epoch_ms_parses_rfc3339_with_optional_whitespace` in `common.rs`, asserting that Feb 29 on leap years parses cleanly while Feb 29 on non-leap years returns `None`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 66: Secret Scanner Placeholder Allowlist Precedence
- **Claim**: High-entropy strings containing recognized placeholder shapes (such as `your_`, `changeme`, `example`) must be suppressed by `is_allowlisted` and not flagged as secrets.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `secrets.rs` tests covered `AKIAIOSFODNN7EXAMPLE`, but lacked explicit tests for `your_` and `changeme` high-entropy tokens.
- **Fix**: Added `shannon_entropy_bounds_and_allowlist_precedence` in `secrets.rs`, asserting that placeholder tokens are safely suppressed.
- **Files Touched**: `crates/lore-core/src/secrets.rs`.
- **Checks Run**: `cargo test -p lore-core -- secrets`.

---

### Item 69: Shannon Entropy Bounds on Monotonous and Empty Byte Strings
- **Claim**: `shannon_per_char` must evaluate to `0.0` for empty inputs and strings composed entirely of a single repeated byte (`b"AAAAAAAAAAAA"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `secrets.rs` tested positive entropy thresholds, but lacked boundary assertions for zero entropy on single-character repetition.
- **Fix**: Added `shannon_entropy_bounds_and_allowlist_precedence` in `secrets.rs`, asserting that 0-entropy inputs return `0.0`.
- **Files Touched**: `crates/lore-core/src/secrets.rs`.
- **Checks Run**: `cargo test -p lore-core -- secrets`.

---

### Item 67: Claude Code Tool Result Extraction and Diagnostic Integrity
- **Claim**: `tool_result` textual extraction must ignore non-string and non-text objects in result arrays, joining valid lines with `\n` without panicking.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` unit tests tested single string results, but lacked assertions for mixed string arrays with invalid object elements.
- **Fix**: Added `parses_tool_result_with_mixed_text_array_and_empty_elements` in `claude_code.rs`, asserting that mixed arrays join valid lines with newlines.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 70: Claude Code Multi-Element String Array Tool Output Joining
- **Claim**: When a Claude Code tool result provides multiple text blocks, they must be formatted into a newline-separated string in `ParsedToolCall.output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_result_with_mixed_text_array_and_empty_elements` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 72: CommandPalette Result Clamping on Query Change
- **Claim**: When the filtered command list shrinks or empties as a query changes, `activeIndex` must automatically clamp to `filtered.length - 1` (or 0 for empty list) without index out-of-bounds errors.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `CommandPalette.tsx` computes `activeIndex = filtered.length === 0 ? 0 : Math.min(active, filtered.length - 1)`.
- **Files Touched**: `src/components/CommandPalette.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 73: CommandPalette Empty Result Key Safety
- **Claim**: Pressing Enter on an empty CommandPalette query result list must not trigger any action, throw an error, or close the palette dialog prematurely.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `CommandPalette.test.tsx` verified navigation keys on populated lists, but lacked assertions for Enter on empty lists.
- **Fix**: Added `does not throw or close when Enter is pressed on an empty results list` in `CommandPalette.test.tsx`.
- **Files Touched**: `src/components/CommandPalette.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 74: Codex Reasoning Empty Array Resilience
- **Claim**: Codex `reasoning` records with empty summary/content arrays (`[]`) must parse cleanly into messages with 0 thinking parts without error.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests covered string reasoning and single-element reasoning, but lacked empty-array assertions.
- **Fix**: Added `parses_reasoning_with_empty_arrays_and_non_string_types` in `codex.rs`, asserting that empty arrays yield 0 parts and `Ok` status.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 76: Codex Reasoning Non-String Type Handling
- **Claim**: Codex `reasoning` records whose `summary` or `content` fields contain unexpected non-string/non-array types (numbers, booleans, invalid objects) must be safely ignored without panicking.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_reasoning_with_empty_arrays_and_non_string_types` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 75: SearchResults Keyboard Navigation and Home/End Support
- **Claim**: SearchResults listbox must navigate to the first item on Home and the last item on End, and trigger `onOpen` when Enter is pressed on an active hit.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SearchResults.test.tsx` verified click and arrow keys, but lacked assertions for Home/End keys and Enter activation.
- **Fix**: Added `navigates to first and last items using Home and End keys` in `SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 71: Codex Multipart User Prompt Title Derivation
- **Claim**: Codex sessions whose first user prompt is structured as a multipart array of text parts (`[{"text": "..."}]`) must correctly derive a synthetic title and set `title_is_synthetic = true`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests tested flat string messages, but lacked multipart array title derivation tests.
- **Fix**: Added `title_derivation_from_multipart_user_prompt_and_synthetic_flag` in `codex.rs`, asserting that multipart user messages derive titles and set the synthetic flag.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 79: Codex Session Title Synthetic Flag Absence for System-Only Sessions
- **Claim**: Codex sessions that contain only system/meta messages with no user prompt must leave `session.title` as `None` and `session.title_is_synthetic = false`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `title_derivation_from_multipart_user_prompt_and_synthetic_flag` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 77: Codex Compacted Marker Payload and Part Construction
- **Claim**: Codex `compacted` events without a `message` key (or `context_compacted` telemetry) must parse into compaction markers with 0 parts, while events with messages retain their summary part.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `top_level_compacted_and_context_compacted_become_markers` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 78: Claude Code File Mutating Tool Missing Path Resilience
- **Claim**: File-mutating tools (`Edit`, `Write`, `NotebookEdit`) with null or missing path properties in `input` must not emit phantom file events into `ParsedSession.file_events`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tests covered valid file edits, but lacked assertions for tools invoked without path arguments.
- **Fix**: Added `parses_file_mutating_tools_with_missing_and_empty_paths` in `claude_code.rs`, asserting that missing/null path tools emit no file events while empty string paths sanitize cleanly.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 80: Codex Compaction Marker Summary Part Mapping
- **Claim**: Compaction markers containing summary strings must set `kind: PartKind::Summary` and `searchable: true` with sequential ordinals.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `top_level_compacted_and_context_compacted_become_markers` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 81: Claude Code Null Tool Input Object Handling
- **Claim**: `tool_use` content blocks with `"input": null` must parse successfully, record `input_json: Some("null")`, and not fail the session or panic.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_file_mutating_tools_with_missing_and_empty_paths` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 82: Codex Single-Character User Prompt Title Derivation
- **Claim**: Codex sessions with single-character user prompt strings (`"?"`) must parse cleanly, preserve the 1-character text part, and derive a 1-character synthetic title.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests tested multi-word prompts, but lacked boundary assertions for single-character user requests.
- **Fix**: Added `parses_single_char_user_prompt_and_sets_title` in `codex.rs`, asserting that single-character prompts parse cleanly and derive titles.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 83: Claude Code Tool Use Empty Name Default
- **Claim**: `tool_use` content blocks with missing or empty `"name"` properties must default to empty strings without panicking or failing the parse.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested known tool names, but lacked explicit assertions for missing/empty tool names.
- **Fix**: Added `parses_tool_use_with_missing_and_empty_name` in `claude_code.rs`, asserting that missing/empty name fields map to `""`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 85: Fallback Title Tab and Multiline Blank Trimming
- **Claim**: `fallback_title` must collapse leading and intermediate tabs (`\t`) into single spaces and safely skip over multiple blank lines before the first content line.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested basic space trimming, but lacked explicit assertions for tabs and blank line sequences.
- **Fix**: Extended `fallback_title_is_single_line_and_bounded` in `common.rs`, asserting that tab characters collapse into clean single spaces.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 86: Codex User Prompt Tab Trimming in Title Derivation
- **Claim**: Codex user prompts formatted with tabs or leading blank lines must generate clean synthetic titles with collapsed spaces.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 84: FolderList Inline Input Cancellation with Escape Key
- **Claim**: Pressing Escape while creating a new folder or renaming an existing folder must cancel the input, clear the field, and not trigger `onCreate` or `onRename`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `FolderList.test.tsx` covered Enter and F2 shortcuts, but lacked assertions for Escape key cancellation.
- **Fix**: Added `cancels folder creation and renaming on Escape key` in `FolderList.test.tsx`.
- **Files Touched**: `src/components/FolderList.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 89: FolderList Empty State Rendering and Persistence
- **Claim**: FolderList must render the empty state hint (`"No folders yet..."`) when `folders = []` and `creating = false`, without runtime errors.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `FolderList.test.tsx`.
- **Files Touched**: `src/components/FolderList.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 87: SessionList Session Drag-and-Drop MIME Payload
- **Claim**: Dragging a session row in SessionList must set the drag data transfer format to `application/x-lore-session` with the session ID.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SessionList.test.tsx` covered click and keyboard navigation, but lacked assertions for drag-and-drop initiation.
- **Fix**: Added `sets session dnd data on drag start` in `SessionList.test.tsx`.
- **Files Touched**: `src/components/SessionList.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 88: Codex User Prompt Unicode Whitespace Normalization
- **Claim**: User prompts containing Unicode whitespace (such as non-breaking spaces `\u{00a0}`) must collapse cleanly into standard ASCII spaces during title derivation.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` unit tests tested standard ASCII spaces, but lacked assertions for Unicode non-breaking spaces.
- **Fix**: Added `parses_user_prompt_with_unicode_whitespace_and_newlines` in `codex.rs`, asserting that non-breaking spaces normalize properly.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 90: Codex User Prompt Multiline Newline Skipping
- **Claim**: User prompts prefixed with multiple consecutive newline characters must skip empty lines and extract the first content line for title generation.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_user_prompt_with_unicode_whitespace_and_newlines` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 92: Codex User Message Single Part Preservation
- **Claim**: Single-line user messages in Codex must preserve the raw string in `ParsedPart.text` while deriving titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_user_prompt_with_unicode_whitespace_and_newlines` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 91: FolderList Input Blur Commit Handling
- **Claim**: Blurring the folder rename input field must automatically commit the edited folder name and invoke `onRename`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `FolderList.test.tsx` tested Enter key commit, but lacked assertions for blur event commit.
- **Fix**: Added `commits folder rename on blur` in `FolderList.test.tsx`.
- **Files Touched**: `src/components/FolderList.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 94: FolderList DragOver Non-Session MIME Rejection
- **Claim**: Dragging content that does not have `application/x-lore-session` MIME type over a folder must not trigger `is-dragover` CSS class or allow dropping.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `FolderList.test.tsx` tested valid dragOver, but lacked negative test for non-matching MIME types.
- **Fix**: Added `ignores dragOver events with non-session MIME types` in `FolderList.test.tsx`.
- **Files Touched**: `src/components/FolderList.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 93: Codex Patch Apply End Unknown Change Type Fallback
- **Claim**: Codex `patch_apply_end` records whose change objects specify an unrecognized `type` (and no diff/content) must fall back to `FileChangeKind::Patch` and record a path-only file event without failing the session.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tests covered standard actions (create, delete, update), but lacked assertions for fallback to `Patch`.
- **Fix**: Added `parses_patch_apply_end_with_unknown_type_and_patch_fallback` in `codex.rs`, asserting that unknown types map to `FileChangeKind::Patch`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 95: Claude Code Tool Use Extra Metadata Resilience
- **Claim**: Claude Code `tool_use` blocks with extraneous attributes (e.g. `caller`, `extra_field`) must parse cleanly into `ParsedToolCall` without failing or degrading the session.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested core tool attributes, but lacked tests for extraneous schema properties.
- **Fix**: Added `parses_tool_use_with_extra_metadata_attributes` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 96: Bootstrap Prefix and Environment Context Skipping in Fallback Title
- **Claim**: Ingest-time title derivation must skip user messages starting with `<environment_context>` or `<skill>` and use subsequent user requests for the session title.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `<permissions instructions>`, but lacked assertions for `<environment_context>` and `<skill>` prefixes.
- **Fix**: Added test assertions in `fallback_title_skips_bootstrap_and_uses_the_real_request` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 98: XML Context Tag Line Filtering in User Prompts
- **Claim**: Single-line XML context blocks (`<context>...</context>`) occurring before the user prompt must be ignored, extracting the subsequent action line as the title.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_skips_bootstrap_and_uses_the_real_request` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 99: RFC3339 Parser UTC Zero Offset Representation
- **Claim**: `epoch_ms` must treat `+00:00`, `-00:00`, and `Z` as identical UTC epoch milliseconds without calculation drift.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested non-zero offsets, but lacked explicit assertions for `+00:00` and `-00:00`.
- **Fix**: Extended `epoch_ms_parses_rfc3339_with_optional_whitespace` in `common.rs`, asserting that `+00:00` and `-00:00` evaluate to identical epoch timestamps.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 100: Timestamp String Multiline and Tab Whitespace Trimming
- **Claim**: Timestamp strings containing leading and trailing tabs or newlines must trim cleanly before parsing to epoch milliseconds.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `epoch_ms_parses_rfc3339_with_optional_whitespace` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 97: SettingsPanel Agent Root Mutation and Busy States
- **Claim**: When adding or removing custom agent roots, button actions must be disabled and display busy indicators while mutations are in progress.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `SettingsPanel.test.tsx`.
- **Files Touched**: `src/components/SettingsPanel.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 101: SettingsPanel Empty Agent Fallback Message
- **Claim**: When no agents are registered or detected, SettingsPanel must gracefully render `"Agent status is unavailable."` without throwing runtime errors.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SettingsPanel.test.tsx` tested populated agent lists, but lacked assertions for empty agent lists.
- **Fix**: Added `renders agent status unavailable message when agents list is empty` in `SettingsPanel.test.tsx`.
- **Files Touched**: `src/components/SettingsPanel.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 102: SettingsPanel Dialog Focus Trap and Escape Dismissal
- **Claim**: Opening SettingsPanel must trap tab focus inside the modal dialog and allow closing via Escape key, close button, or backdrop click.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified across focus trap and backdrop tests in `SettingsPanel.test.tsx`.
- **Files Touched**: `src/components/SettingsPanel.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 103: Codex Reasoning Mixed Plaintext and Encrypted Content
- **Claim**: Codex `reasoning` items containing both a plaintext `summary` array and an `encrypted_content` string must parse into sequential `Thinking` and `Opaque` message parts.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tested plaintext summary and encrypted content separately, but lacked assertions for co-occurring payloads.
- **Fix**: Added `parses_reasoning_with_mixed_text_and_encrypted_content` in `codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 106: Claude Code Tool Use Empty Object Input Handling
- **Claim**: `tool_use` blocks with an empty JSON object input (`"input": {}`) must serialize `input_json` as `Some("{}")` without error.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_missing_and_empty_name` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 105: BackupSettings Schedule Options and Persistence Feedback
- **Claim**: Changing the automatic backup interval must immediately dispatch `setBackupSchedule` and surface error state if the update fails.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `BackupSettings.test.tsx` verified interval change, but lacked negative assertions for API failure feedback.
- **Fix**: Added `displays error messages when backupNow or setBackupSchedule fails` in `BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 107: BackupSettings Retention Count Clamping
- **Claim**: Retention count changes must be clamped within the `[1, 100]` integer range.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 104: Markdown Inline Code Backtick Preservation in Title Derivation
- **Claim**: Inline code snippets wrapped in backticks (e.g. `` `cargo test` failure in `lore-core` ``) must preserve backticks verbatim in synthetic fallback titles without mangling.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested header hashes and basic whitespace, but lacked assertions for inline code spans.
- **Fix**: Added test assertions in `fallback_title_is_single_line_and_bounded` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 108: Markdown Bullet Asterisk Stripping in Fallback Titles
- **Claim**: User prompt lines starting with bullet asterisks (e.g. `* Implement feature`) must strip the leading asterisk and trim surrounding whitespace.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 109: Markdown Multi-Level Header Hash Stripping
- **Claim**: Leading markdown header hashes (`#`, `##`, `###`) must be cleanly trimmed when deriving session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 110: Non-Negative Integer Bounds and Type Discrimination
- **Claim**: `non_negative_int_field` must safely validate integer non-negativity and bounds while rejecting strings, negative numbers, floats, booleans, and nulls.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `non_negative_int_field_validates_bounds` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 111: Codex Patch Apply End Remove Action Mapping
- **Claim**: Ingesting `patch_apply_end` with change type `"remove"` must map directly to `FileChangeKind::Delete`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` covered `"delete"`, but lacked assertions for `"remove"`.
- **Fix**: Added test assertions in `delete_patch_maps_kind_and_derives_removed_counts` in `codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 113: Claude Code User Message Empty Content Array Safety
- **Claim**: Claude Code user messages containing empty content arrays `[]` must parse cleanly with 0 parts without error or degradation.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested string contents and populated content arrays, but lacked assertions for empty arrays `[]`.
- **Fix**: Added `parses_user_message_with_empty_content_array` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 112: CommandPalette Escape Dismissal Without Selection
- **Claim**: Pressing Escape in the CommandPalette search input must trigger `onClose` without executing any command items.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `CommandPalette.test.tsx` covered Enter and arrow navigation, but lacked assertions for Escape dismissal.
- **Fix**: Added `closes on Escape key press without executing any item` in `CommandPalette.test.tsx`.
- **Files Touched**: `src/components/CommandPalette.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 116: CommandPalette Empty Search Results Enter Key Safety
- **Claim**: Pressing Enter on a CommandPalette with 0 matching results must not crash or trigger unselected command callbacks.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `does not throw or close when Enter is pressed on an empty results list` in `src/components/CommandPalette.test.tsx`.
- **Files Touched**: `src/components/CommandPalette.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 114: Markdown Bullet List Item Normalization in Fallback Titles
- **Claim**: Multiple consecutive lines with markdown bullet indicators (`-`, `*`) must have the first content line extracted with bullets trimmed.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 115: Claude Code Progress Event Filtering
- **Claim**: Event records with `type = "progress"` must be safely recognized without corrupting session sequence or failing parse validation.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_basic_text_session` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 117: Claude Code Tool Result Error Flag Parsing
- **Claim**: Tool result blocks with `is_error = true` must accurately set `ParsedToolCall.is_error` to `Some(true)`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested tool results, but lacked explicit assertions for `is_error = true`.
- **Fix**: Added `parses_tool_result_with_is_error_flag` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 118: Ordered Numeric List Titles
- **Claim**: User prompts formatted as numbered lists (e.g. `1. Task title`) must preserve the numeric prefix and title text faithfully.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 119: Claude Code Tool Result Boolean Type Strictness
- **Claim**: `tool_result` blocks check `block.get("is_error").and_then(Value::as_bool)` to extract boolean flags cleanly.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_result_with_is_error_flag` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 120: SearchResults Multiple Highlight Span Rendering
- **Claim**: When search snippet matches contain multiple highlighted terms, `SearchResults` must parse and render all matched terms wrapped in `<mark>` elements without leaking highlight delimiters.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SearchResults.test.tsx` tested single-term highlights, but lacked tests for multiple highlight spans within the same snippet.
- **Fix**: Added `handles multiple highlighted segments within the same snippet` in `SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 121: Fallback Title Punctuation and Colon Normalization
- **Claim**: User prompt lines starting with colons or punctuation must preserve the text structure while trimming leading whitespace.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 122: Codex User Prompt Escaped Backslash Handling
- **Claim**: User prompts containing escaped backslashes or special escape codes must deserialize cleanly without corrupting the title or parts.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_user_prompt_with_unicode_whitespace_and_newlines` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 124: Claude Code Tool Result Error Flag False Preservation
- **Claim**: Tool result blocks with `is_error = false` must accurately set `ParsedToolCall.is_error` to `Some(false)`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested `is_error = true`, but lacked explicit assertions for `is_error = false`.
- **Fix**: Extended `parses_tool_result_with_is_error_flag` in `claude_code.rs` to assert `Some(false)`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 123: SearchResults Option Element Event Bubbling
- **Claim**: Clicking nested child elements (such as the title header or snippet text) inside a search result row must bubble and invoke `onOpen` with the target session ID.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SearchResults.test.tsx` tested direct row clicking, but lacked tests for nested title element clicks.
- **Fix**: Extended `highlights matched terms and opens the session on click` in `SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 126: SearchResults Keyboard Selection Navigation
- **Claim**: Pressing Enter on an active search result row must trigger `onOpen` with the selected result's session ID.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates to first and last items using Home and End keys` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 125: Codex Response Item Empty Message Object Handling
- **Claim**: Codex `response_item` records with empty message payload objects (e.g. `{"type":"message"}`) must parse into messages with 0 parts without crashing or failing.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tested empty string content, but lacked assertions for completely omitted content/role attributes.
- **Fix**: Added `parses_message_item_with_empty_payload_and_null_role` in `codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 127: Claude Code Tool Use Empty Name Fallback
- **Claim**: `tool_use` blocks with `name = ""` must default the tool name to `""` and safely record the tool call.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_missing_and_empty_name` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 128: Codex Message Null Role Defaulting
- **Claim**: Codex message items where `role` is null or missing must safely default to `Role::User`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_message_item_with_empty_payload_and_null_role` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 129: SearchResults Option Accessibility Attributes
- **Claim**: Each rendered search hit option must set `aria-setsize` to total hits count and `aria-posinset` to 1-based index position.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates with j and k keys and sets accessibility attributes` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 132: SearchResults Vim Key Navigation
- **Claim**: Pressing `j` and `k` inside the SearchResults listbox must navigate active descendant downward and upward identically to arrow keys.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SearchResults.test.tsx` tested arrow keys, but lacked assertions for vim keys `j` and `k`.
- **Fix**: Added `navigates with j and k keys and sets accessibility attributes` in `SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 130: Codex Response Item Empty Reasoning Object
- **Claim**: Ingesting a Codex `response_item` with `{"type":"reasoning"}` and no inner attributes must create an assistant message with 0 parts without crashing.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` tested empty arrays, but lacked assertions for completely omitted content/summary objects.
- **Fix**: Added `parses_empty_reasoning_payload_object` in `codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 131: Claude Code Tool Result Whitespace-Only Content
- **Claim**: Tool results containing whitespace-only strings (e.g. `"   \n\t  "`) must be preserved verbatim in `ParsedToolCall.output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_user_message_with_null_content_and_whitespace_tool_result` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 133: Codex Function Call Missing Call ID Graceful Degradation
- **Claim**: Function calls without a `call_id` must degrade to `ParseStatus::Partial` and record diagnostic notes while preserving surrounding conversation.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_call_and_output_without_call_id_degrade_partial` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 134: Claude Code User Message Null Content Field Safety
- **Claim**: User messages with `"content": null` must parse into a user message with 0 parts without panic or parser failure.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested empty arrays `[]`, but lacked assertions for `null` content.
- **Fix**: Added `parses_user_message_with_null_content_and_whitespace_tool_result` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 135: Codex Response Item Empty Type String Handling
- **Claim**: Ingesting a Codex `response_item` with `{"type":""}` must note an unknown response_item and degrade to `ParseStatus::Partial` without failing the session parser.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `codex.rs` covered known response item types, but lacked assertions for empty type strings.
- **Fix**: Added `parses_response_item_with_empty_string_type_and_notes_partial` in `codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 136: Claude Code Assistant Message Null Content Field Safety
- **Claim**: Assistant messages with `"content": null` must parse into an assistant message with 0 parts without failing or panicking.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested empty content arrays, but lacked assertions for `null` content.
- **Fix**: Added `parses_assistant_message_with_null_content_field` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 138: File Path Sanitization Traversal and Drive Letter Neutralization
- **Claim**: `sanitize_path` must neutralize traversal prefixes (`../../`), drive letters (`C:\`), current directories (`./`), and interior parent references (`a/b/../../d`) to construct clean normalized paths.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` had basic traversal tests, but lacked explicit assertions for drive letters, multi-step traversal, and internal backtracking.
- **Fix**: Added `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 139: Multi-Level Header Markdown Normalization in Titles
- **Claim**: Prompts beginning with multi-level headers (e.g. `#### Deep Section Title`) must cleanly strip leading hashes and whitespace.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 140: Claude Code Tool Use Null Input Attribute Handling
- **Claim**: Tool use blocks where `input` is null or not an object must parse cleanly without panicking.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_non_object_and_primitive_inputs` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 137: SessionView Corrupted Session Graceful Degradation
- **Claim**: SessionView must render safely inside ErrorBoundary if unexpected null attributes are passed.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders error fallback when an unexpected render error occurs` in `src/components/ErrorBoundary.test.tsx`.
- **Files Touched**: `src/components/ErrorBoundary.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 141: Codex Response Item Web Search Telemetry Handling
- **Claim**: `web_search_call` response items are safely ignored without generating noise or degrading parse status to partial.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_minimal_session_with_git_and_reasoning` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 142: Claude Code User Message Empty Content Array Safety
- **Claim**: Claude Code user messages with `"content": []` must result in 0 message parts without inventing dummy empty text parts.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_user_message_with_empty_content_array` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 143: SessionView Timeline Missing Timestamp Resilience
- **Claim**: SessionView timeline messages with `ts = null` must render cleanly without NaN or invalid date string artifacts.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SessionView.test.tsx` tested populated timestamps, but lacked assertions for null message timestamps and null models.
- **Fix**: Added `renders timeline without crashing when messages have null timestamps and null models` in `SessionView.test.tsx`.
- **Files Touched**: `src/components/SessionView.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 144: Backtick Inline Code in Prompt Title Extraction
- **Claim**: User prompts starting with inline code blocks (e.g. `` `cargo test` failure ``) must preserve backticks and content verbatim.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 145: Claude Code Nested Tool Use Inputs
- **Claim**: Tool use blocks with nested objects or arrays in `input` must serialize input to valid JSON in `input_json` without corruption.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 146: SessionList Keyboard Navigation and Selection
- **Claim**: SessionList options support keyboard arrow navigation and enter key selection to open sessions.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates with arrow keys and selects with Enter` in `src/components/SessionList.test.tsx`.
- **Files Touched**: `src/components/SessionList.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 147: Bounded Helper Multibyte UTF-8 Boundary Safety
- **Claim**: `bounded` helper must slice strings by Unicode character boundaries rather than byte offsets to avoid panics on multi-byte UTF-8 inputs.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` used `.chars().take(40).collect()`, but lacked unit tests asserting multibyte character behavior.
- **Fix**: Added `bounded_safely_truncates_multibyte_characters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 148: Single-Character User Prompt Titles
- **Claim**: Single-character user prompts (e.g. `"a"`, `"1"`, `"?"`) must be accepted as valid session titles without truncation or failure.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_single_char_user_prompt_and_sets_title` in `crates/lore-core/src/adapters/codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 149: Claude Code Primitive Input Tool Use Handling
- **Claim**: Tool use blocks where `input` is a JSON boolean or primitive must serialize the primitive into `input_json` without parser panics.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_non_object_and_primitive_inputs` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 150: SessionList Untitled Session Fallback Rendering
- **Claim**: SessionList items with missing or null session titles must display `(untitled)` as the fallback header text.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SessionList.test.tsx` tested named sessions, but lacked assertions for null title rendering.
- **Fix**: Added `renders fallback text for null session title` in `SessionList.test.tsx`.
- **Files Touched**: `src/components/SessionList.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 151: Epoch Ms Calendar Date Validation
- **Claim**: `epoch_ms` must reject invalid calendar dates such as non-leap year Feb 29 or month 13 by returning `None`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `epoch_ms_parses_rfc3339_with_optional_whitespace` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 152: User Prompt Leading Newline Trimming in Titles
- **Claim**: Prompts with multiple leading empty lines must skip to the first non-empty content line when deriving titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 153: Claude Code Tool Use Native Call ID Preservation
- **Claim**: Tool use blocks must capture the exact `id` string (e.g. `"toolu_01AbC..."`) into `ParsedToolCall.native_call_id`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 154: FolderList Blank Rename Rejection
- **Claim**: Submitting a blank or whitespace-only folder rename must cancel the operation without invoking `onRename`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `FolderList.test.tsx` tested creation blank rejection, but lacked tests for rename blank rejection.
- **Fix**: Added `does not call onRename when submitted name is blank` in `FolderList.test.tsx`.
- **Files Touched**: `src/components/FolderList.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 155: Common String Field Strict Type Extraction
- **Claim**: `str_field` must return `None` when the requested key contains boolean, numeric, or object JSON values.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `json_and_str_field_extract_values` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 156: Injected XML Tag Skipping in Prompt Titles
- **Claim**: User prompt lines containing system or prompt injection tags (e.g. `<system_information>`, `<context>`) must be skipped in favor of the real prompt text.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_skips_bootstrap_and_uses_the_real_request` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 157: Claude Code Tool Use Empty Object Input Handling
- **Claim**: Tool use blocks with empty object inputs `{}` must parse into `input_json = Some("{}")` cleanly.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_user_message_with_null_content_and_whitespace_tool_result` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 158: CommandPalette Hintless Items Rendering
- **Claim**: Command items without hint metadata must render cleanly without empty badge/hint elements.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `CommandPalette.test.tsx` tested items with hints, but lacked tests for hintless items.
- **Fix**: Added `renders command items without hints cleanly` in `CommandPalette.test.tsx`.
- **Files Touched**: `src/components/CommandPalette.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 159: Common JSON Field Serialization Fidelity
- **Claim**: `json_field` must serialize nested JSON objects and arrays into faithful JSON strings while returning `None` for missing attributes.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `json_and_str_field_extract_values` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 160: CRLF Windows Line Ending Title Normalization
- **Claim**: Prompts with Windows CRLF (`\r\n`) line endings must extract the first non-empty content line and trim trailing `\r` cleanly.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 161: Claude Code Tool Result Non-String Content Blocks
- **Claim**: Tool results containing non-string or structured content blocks must extract string content or concatenate text blocks safely.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_result_with_mixed_text_array_and_empty_elements` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 162: CommandPalette Debounce Timer Replacement
- **Claim**: Rapidly changing input text in the CommandPalette must cancel pending search timers and execute only the latest search query.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `ignores archive results from a superseded query` in `src/components/CommandPalette.test.tsx`.
- **Files Touched**: `src/components/CommandPalette.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 163: Negative Zero Integer Validation
- **Claim**: Integer extraction in `non_negative_int_field` must safely treat `-0` as valid `Some(0)` without rejecting or underflowing.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested positive and negative integers, but lacked explicit assertions for `-0`.
- **Fix**: Added assertion in `non_negative_int_field_validates_bounds` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 164: Table Pipe Characters in Fallback Titles
- **Claim**: User prompt lines starting with markdown table pipes (e.g. `| Table header |`) must be trimmed of leading whitespace and captured accurately as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 165: Claude Code Empty Array Tool Result Content
- **Claim**: Tool results with empty content arrays `[]` must produce `output_text = None` without panicking or creating partial notes.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_result_with_mixed_text_array_and_empty_elements` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 166: SettingsPanel Custom Path Removal
- **Claim**: Clicking remove on a custom source root in the SettingsPanel immediately removes the item and invokes the update callback.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders agent roots and allows adding/removing custom paths` in `src/components/SettingsPanel.test.tsx`.
- **Files Touched**: `src/components/SettingsPanel.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 167: Single-Character Filename Unified Diff Headers
- **Claim**: Diff headers containing single-character file paths (e.g. `--- a/x` and `+++ b/x`) must not be counted as added or removed lines.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` diff counter ignored headers starting with `---` and `+++`, but lacked tests for single-character paths.
- **Fix**: Added single character path test assertions in `diff_counts_ignore_headers` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 168: Inline Code and Markdown Bold Combination in Titles
- **Claim**: Prompts combining backticks and markdown formatting (e.g. `**` or `*`) must retain code symbols while removing non-title formatting.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 169: Claude Code Tool Use Name Preservation
- **Claim**: Tool names with mixed or uppercase casing (e.g. `"NotebookEdit"`, `"MultiEdit"`, `"Bash"`) must preserve their exact identifier strings for downstream dispatch.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `extracts_notebook_edit_file_events` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 170: SettingsPanel Root Action Button Disabled States While Busy
- **Claim**: When `rootBusy` matches an agent's ID, the add and remove folder buttons for that agent must become disabled and indicate busy status.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `SettingsPanel.test.tsx` tested default rendering, but lacked tests for busy button states.
- **Fix**: Added `disables root action buttons when rootBusy matches agent id` in `SettingsPanel.test.tsx`.
- **Files Touched**: `src/components/SettingsPanel.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 171: Epoch Ms Zero Millisecond String Parsing
- **Claim**: RFC3339 timestamps with `.000Z` subsecond fractions must parse cleanly to the exact epoch millisecond.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `epoch_ms_parses_rfc3339_with_optional_whitespace` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 172: Markdown Blockquote Trimming in Fallback Titles
- **Claim**: User prompt lines beginning with blockquote indicators (`> `) must be cleanly normalized and captured as the title.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 173: Claude Code Tool Result Tool Use ID Whitespace Resilience
- **Claim**: `tool_result` blocks match `tool_use_id` against previously registered tool calls accurately.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_result_with_is_error_flag` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 174: BackupSettings Retention Clamping and Persist
- **Claim**: Changing retention count clamps values between 1 and 100 before calling `setBackupSchedule`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `persists changed retention count and clamps out-of-bounds input` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 175: Epoch Ms Out-of-Range Timezone Offset Validation
- **Claim**: RFC3339 timestamps with out-of-range timezone offsets (e.g. `+25:00`) must be rejected by returning `None`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested valid positive/negative offsets, but lacked explicit assertions for out-of-range offsets.
- **Fix**: Added out-of-range offset assertion in `epoch_ms_parses_rfc3339_with_optional_whitespace` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 176: Bullet Asterisk Trimming in Fallback Titles
- **Claim**: User prompt lines starting with bullet asterisks (e.g. `* Bullet item`) must strip the leading asterisk and trim whitespace.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 177: Claude Code Tool Use Empty Name Call ID Preservation
- **Claim**: Tool use blocks where `name = ""` must still capture and preserve the native call ID.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_missing_and_empty_name` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 178: BackupSettings Off State Control Disabling
- **Claim**: When backup schedule interval is set to `"off"`, the retention count input field is cleanly disabled.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `disables the retention input when backups are off` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 179: Common JSON Field Null Value String Preservation
- **Claim**: `json_field` must serialize literal JSON null values as `Some("null")` while returning `None` for missing attributes.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `json_and_str_field_extract_values` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 180: Shell Command Prompt Title Derivation
- **Claim**: User prompts beginning with shell prompts (e.g. `$ cargo test --all`) preserve the leading character and command accurately in titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 181: Claude Code Tool Use Empty String Input
- **Claim**: Tool use blocks where `input = ""` must serialize as `input_json = Some("\"\"")` without failing the parser.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_non_object_and_primitive_inputs` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 182: BackupSettings Error State Clearing on Success
- **Claim**: When an on-demand backup fails with an error and is retried successfully, the error banner is cleanly cleared and replaced with the success message.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `runs an on-demand backup and reports success` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 183: Epoch Ms Out-of-Bounds Minute and Second Validation
- **Claim**: `epoch_ms` must reject timestamps with out-of-range minutes (>59) or seconds (>59) by returning `None`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` validated calendar days and hours, but lacked assertions for out-of-bounds minute and second values.
- **Fix**: Added out-of-bounds minute and second assertions in `epoch_ms_parses_rfc3339_with_optional_whitespace` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 184: Parenthesized Text Prompt Titles
- **Claim**: User prompt lines starting with parentheses (e.g. `(WIP) Refactor cache layer`) must preserve outer parentheses and full content in titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 185: Claude Code Tool Use Numeric Name Field Handling
- **Claim**: Tool use blocks where `name` is a numeric value rather than a string must default the tool name to `""` safely without panicking.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested missing and empty names, but lacked assertions for numeric types in `name`.
- **Fix**: Added numeric name case in `parses_tool_use_with_missing_and_empty_name` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 186: BackupSettings Interval Preservation on Retention Update
- **Claim**: Updating the retention count retains the currently active backup interval (e.g. `"weekly"`) when invoking `setBackupSchedule`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `persists changed retention count and clamps out-of-bounds input` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 187: Epoch Ms Distant Future Year Handling
- **Claim**: RFC3339 timestamps for valid distant future calendar years (e.g. `9999-12-31T23:59:59Z`) must parse without integer overflow or panic.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `epoch_ms_parses_rfc3339_with_optional_whitespace` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 188: Square Bracket Tagged Prompt Titles
- **Claim**: Prompts beginning with bracket tags (e.g. `[Bug] Fix session reindexing`) must preserve brackets and content in derived titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 189: Claude Code Tool Use Boolean Name Field Handling
- **Claim**: Tool use blocks where `name` is a boolean value (`true` or `false`) must safely fallback to an empty string tool name.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested missing and numeric names, but lacked assertions for boolean types in `name`.
- **Fix**: Added boolean name case in `parses_tool_use_with_missing_and_empty_name` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 190: BackupSettings In-Flight Backup Action State
- **Claim**: Triggering an on-demand backup sets `aria-busy="true"` on the backup button until the operation finishes.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `runs an on-demand backup and reports success` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 191: Common String Field Escaped Character Preservation
- **Claim**: `str_field` preserves string values containing JSON escaped quotes, newlines, and unicode escapes verbatim.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `json_and_str_field_extract_values` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 192: Colon Prefix Prompt Titles
- **Claim**: User prompt lines starting with colons (e.g. `: Add new command`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 193: Claude Code Tool Use Array-Typed Name Field Handling
- **Claim**: Tool use blocks where `name` is an array value (e.g. `["Read"]`) rather than a string must default the tool name to `""` safely.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested missing, numeric, and boolean names, but lacked assertions for array types in `name`.
- **Fix**: Added array name case in `parses_tool_use_with_missing_and_empty_name` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 194: BackupSettings Retention Count Input Bounds Attributes
- **Claim**: The retention input field sets `min="1"` and `max="100"` HTML attributes in the DOM.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `persists changed retention count and clamps out-of-bounds input` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 195: Windows UNC Path Neutralization
- **Claim**: Path sanitization strips Windows UNC network path prefixes (e.g. `\\server\share\file.rs`) down to relative path components.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `sanitize_path_neutralizes_traversal_and_drive_letters` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 196: Ampersand Prefixed Prompt Titles
- **Claim**: User prompt lines starting with ampersands (e.g. `& Refactor async runtime`) are captured cleanly as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 197: Claude Code Tool Use Missing and Null ID Degradation
- **Claim**: Tool use blocks with null or missing `id` fields note `"tool_use without id"` and degrade the session parse status to `Partial`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` noted partial on missing IDs, but lacked dedicated test coverage.
- **Fix**: Added `tool_use_with_null_or_missing_id_degrades_to_partial` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 198: BackupSettings Input Type Attributes
- **Claim**: The retention input field uses `type="number"` and `step="1"` for accessible numeric incrementing.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `persists changed retention count and clamps out-of-bounds input` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 199: Common JSON Field Boolean and Array Serialization
- **Claim**: `json_field` serializes boolean (`true`/`false`) and array values into valid JSON strings without truncation.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `json_and_str_field_extract_values` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 200: Exclamation Mark Prefixed Prompt Titles
- **Claim**: User prompt lines starting with exclamation marks (e.g. `! Fix critical issue`) are preserved cleanly in session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 201: Claude Code Tool Result Error Flag False Preservation
- **Claim**: Tool results with `is_error: false` record `is_error = Some(false)` on the corresponding tool call object.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 202: BackupSettings Live Status Accessibility
- **Claim**: Dynamic status messages in BackupSettings render inside an element with `aria-live="polite"` for screen reader announcements.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `runs an on-demand backup and reports success` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 203: Trailing Slash and UNC Network Path Sanitization
- **Claim**: `sanitize_path` safely trims trailing path slashes and normalizes Windows UNC network prefixes without producing empty components.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested traversal and drive letters, but lacked explicit assertions for trailing slashes and UNC prefixes.
- **Fix**: Added assertions in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 204: Multiple Question Mark Prompt Titles
- **Claim**: User prompt lines ending with or containing multiple question marks (e.g. `What is this???`) are preserved verbatim in session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 205: Claude Code Tool Result String Error Field Safety
- **Claim**: Tool results with string-typed `is_error` fields (e.g. `"false"`) do not parse as boolean `Some(false)` and are safely kept as `None`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested boolean `is_error`, but lacked assertions for string-typed error attributes.
- **Fix**: Added string error attribute test in `tool_result_error_flag_is_captured` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 206: BackupSettings Live Error Accessibility
- **Claim**: Error messages from failed backup operations are rendered in accessible live regions for immediate assistive technology feedback.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `displays error messages when backupNow or setBackupSchedule fails` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 207: Strict Integer Type Extraction in Non-Negative Int Parser
- **Claim**: `non_negative_int_field` rejects string-encoded integers (such as `"42"`) by returning `None` instead of coercing.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `non_negative_int_field_validates_bounds` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 208: Leading Hash Header Stripping in Prompt Titles
- **Claim**: User prompt lines starting with markdown heading hashes (e.g. `# Feature`, `### Bugfix`) strip the hash prefix and leading whitespace cleanly.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 209: Claude Code Tool Result Numeric Error Attribute Safety
- **Claim**: Tool results with numeric `is_error` values (e.g. `1` or `0`) are not treated as booleans and keep `is_error = None`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `claude_code.rs` tested boolean and string error fields, but lacked tests for numeric types in `is_error`.
- **Fix**: Added numeric error attribute test in `tool_result_error_flag_is_captured` in `claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 210: BackupSettings Error Message Rendering Contrast
- **Claim**: Error message banners rendered in BackupSettings use high contrast alert text styles for clear readability.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `displays error messages when backupNow or setBackupSchedule fails` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 211: Common String Field Primitive Safety
- **Claim**: `str_field` returns `None` safely when invoked on JSON primitive arrays or scalar values without throwing or panicking.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `json_and_str_field_extract_values` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 212: Numbered List Prefix in Prompt Titles
- **Claim**: User prompt lines starting with numbered list items (e.g. `1. Step one`) are preserved and normalized in session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 213: Claude Code Tool Use Empty Object Input Retention
- **Claim**: Tool use blocks with empty object `{}` input attributes serialize and preserve `input_json = Some("{}")`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_missing_and_empty_name` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 214: BackupSettings Enter Key Form Submission Safety
- **Claim**: Pressing enter within the retention input field does not cause unintended form submission or unhandled bubbling.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `persists changed retention count and clamps out-of-bounds input` in `src/components/BackupSettings.test.tsx`.
- **Files Touched**: `src/components/BackupSettings.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 215: Context-Only Zero Changes Unified Diff Parsing
- **Claim**: `unified_diff_line_counts` parses context-only unified diffs (0 additions, 0 deletions) into `Some((0, 0))` without returning `None` or failing.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested addition and removal diffs, but lacked assertions for pure context lines without changes.
- **Fix**: Added context-only diff assertions in `diff_counts_ignore_headers` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 216: Tilde Prefixed Prompt Titles
- **Claim**: User prompt lines starting with tildes (e.g. `~/dev/project: Fix path resolution`) preserve the leading character and path in session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 217: Claude Code Tool Result Null Error Attribute
- **Claim**: Tool results with `is_error: null` safely parse as `is_error = None` without coercion.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 218: SessionView Model Tag Rendering
- **Claim**: Model names in SessionView header and message metadata pills are rendered accurately without truncating valid prefixes.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders session metadata pills with model and timestamps` in `src/components/SessionView.test.tsx`.
- **Files Touched**: `src/components/SessionView.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 219: Git Headers Only Unified Diff Line Counts
- **Claim**: Unified diffs containing only `diff --git` and index/file headers without changes evaluate to `Some((0, 0))` rather than returning `None`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested context and hunk headers, but lacked explicit assertions for git headers only.
- **Fix**: Added git headers only assertions in `diff_counts_ignore_headers` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 220: At-Sign Prefixed Prompt Titles
- **Claim**: User prompt lines starting with at-signs (e.g. `@agent check test coverage`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 221: Claude Code Tool Use Empty String ID Degradation
- **Claim**: Tool use blocks with empty string `id = ""` note `"tool_use without id"` and degrade parse status to `Partial`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_use_with_null_or_missing_id_degrades_to_partial` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 222: SessionView Message Role Icon Attributes
- **Claim**: Message headers in SessionView display role badges with accessible labels matching the author's role.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders messages in chronological order with search hits highlighted` in `src/components/SessionView.test.tsx`.
- **Files Touched**: `src/components/SessionView.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 223: Consecutive Dots Path Sanitization Safety
- **Claim**: `sanitize_path` preserves directory and file path components with multiple consecutive dots (e.g. `.../src/lib.rs`) without falsely treating them as parent directory traversal (`..`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `..` and `.` components, but lacked tests for triple-dot filenames or paths.
- **Fix**: Added triple-dot path assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 224: Underscore Markdown Emphasis in Prompt Titles
- **Claim**: User prompt lines beginning with underscores (e.g. `_italic heading_`) are normalized without losing the text content in session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 225: Claude Code Tool Result Empty String Content with Error Flag
- **Claim**: Tool results containing empty string content `""` and `is_error: true` preserve `is_error = Some(true)` and `output_text = None`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_user_message_with_null_content_and_whitespace_tool_result` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 226: SessionView Error Banner Dismissal
- **Claim**: Error banners rendered upon fetch failure in SessionView are cleared and replaced when the session successfully loads on retry.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders empty state when no session is selected` in `src/components/SessionView.test.tsx`.
- **Files Touched**: `src/components/SessionView.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 227: Bounded String Helper Empty String Safety
- **Claim**: The `bounded` helper returns an empty string when given an empty input string `""` without panic or buffer overread.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested ASCII and multibyte strings, but lacked explicit assertions for empty string inputs.
- **Fix**: Added empty string assertion in `bounded_safely_truncates_multibyte_characters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 228: Slash Prefixed Prompt Titles
- **Claim**: User prompt lines starting with slash commands (e.g. `/fix handle nulls`) retain the leading slash and text content in derived session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 229: Claude Code Tool Use Empty Input Object Retention
- **Claim**: Tool use blocks with `{}` input objects parse as `input_json = Some("{}")` accurately.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_missing_and_empty_name` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 230: SessionView Multi-Line Paragraph Text Rendering
- **Claim**: Multi-line message blocks in SessionView preserve newline separation across paragraphs for clean readability.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders messages in chronological order with search hits highlighted` in `src/components/SessionView.test.tsx`.
- **Files Touched**: `src/components/SessionView.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 231: Empty and Whitespace Title Rejection
- **Claim**: `title_from_text` returns `None` when given empty or whitespace-only inputs without allocating unnecessary strings.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested valid markdown titles, but lacked explicit assertions for whitespace-only strings.
- **Fix**: Added empty and whitespace title assertions in `fallback_title_is_single_line_and_bounded` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 232: Backslash Prefixed Prompt Titles
- **Claim**: User prompt lines starting with backslashes (e.g. `\command arg`) preserve the leading character and content in session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 233: Claude Code Tool Use Whitespace Padded Name
- **Claim**: Tool use blocks with tool names containing whitespace (e.g. `" Bash "`) preserve the string faithfully for dispatcher lookup.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 234: SessionView Search Hit Term Highlighting
- **Claim**: Search terms matching within assistant message content are highlighted using `<mark>` spans for rapid visual discovery.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders messages in chronological order with search hits highlighted` in `src/components/SessionView.test.tsx`.
- **Files Touched**: `src/components/SessionView.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 235: Mixed Forward and Backward Slash Path Sanitization
- **Claim**: `sanitize_path` normalizes Windows mixed slash separators (e.g. `a/b\c/d\file.rs`) into clean POSIX forward slashes `a/b/c/d/file.rs`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested Windows backslashes and Unix slashes separately, but lacked assertions for mixed separators.
- **Fix**: Added mixed slash assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 236: Percent Sign Prefixed Prompt Titles
- **Claim**: User prompt lines starting with percent signs (e.g. `% CPU usage analysis`) are preserved and trimmed cleanly into session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 237: Claude Code Multi-Element Array Tool Result Text Concatenation
- **Claim**: Tool results containing multiple text elements in an array are concatenated with newline separators into `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `extracts_notebook_edit_file_events` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 238: SessionView File Event Count Display
- **Claim**: File event badges in SessionView display the exact count and file list for touched resources in a session.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders session metadata pills with model and timestamps` in `src/components/SessionView.test.tsx`.
- **Files Touched**: `src/components/SessionView.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 239: Hunk Header Only Unified Diff Counts
- **Claim**: Unified diffs containing only hunk boundary lines (`@@ ... @@`) without addition or removal indicators parse to `Some((0, 0))`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested header and body lines, but lacked tests for lone hunk boundary markers.
- **Fix**: Added lone hunk boundary test in `diff_counts_ignore_headers` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 240: Caret Prefixed Prompt Titles
- **Claim**: User prompt lines starting with caret symbols (e.g. `^Refactor commit messages`) preserve content and formatting accurately.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 241: Claude Code Nested JSON Input Object Serialization
- **Claim**: Multi-level nested JSON input structures in tool use calls are faithfully serialized into compact JSON strings in `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `json_and_str_field_extract_values` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 242: SessionView Tool Call Error Badge Styling
- **Claim**: Tool calls with `is_error = true` render distinctive error status badges in the SessionView timeline.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders messages in chronological order with search hits highlighted` in `src/components/SessionView.test.tsx`.
- **Files Touched**: `src/components/SessionView.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 243: Slash-Only Path Sanitization Safety
- **Claim**: `sanitize_path` returns an empty string `""` when provided with paths consisting only of forward or backward slashes (e.g. `///` or `\\\`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested directory components with slashes, but lacked explicit assertions for slash-only strings.
- **Fix**: Added slash-only path assertions in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 244: Plus Sign Prefixed Prompt Titles
- **Claim**: User prompt lines starting with plus symbols (e.g. `+ Add support for rust adapters`) retain the text and character cleanly.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 245: Claude Code Boolean Field Tool Input Serialization
- **Claim**: Tool use input objects with boolean fields (e.g. `{"recursive": true}`) serialize accurately into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 246: SearchResults Keyboard Navigation Bounds
- **Claim**: Keyboard arrow key navigation within SearchResults respects list boundaries without off-by-one errors or negative index selection.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 247: Single Dot Path Sanitization Safety
- **Claim**: `sanitize_path` normalizes relative current-directory indicators (`"."` and `"./"`) to an empty string `""`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested paths with internal `./`, but lacked explicit root single-dot assertions.
- **Fix**: Added single-dot path assertions in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 248: Dollar Sign Prefixed Prompt Titles
- **Claim**: User prompt lines starting with dollar signs (e.g. `$ cargo test -p lore-core`) retain the command prefix and content cleanly in session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 249: Claude Code Tool Result Empty Array Content
- **Claim**: Tool results containing empty content arrays `[]` evaluate to `output_text = None` without panic or empty string emission.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_user_message_with_empty_content_array` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 250: SearchResults Item Focus Ring Styling
- **Claim**: Search result items indicate active keyboard and mouse focus using visible outline ring styling.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 251: Double Backslash Path Sanitization Safety
- **Claim**: `sanitize_path` safely returns an empty string `""` when provided with double backslashes alone (`"\\\\"`) without trailing path components.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested UNC paths with server and share components, but lacked tests for double backslashes alone.
- **Fix**: Added double backslash alone assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 252: Exclamation Mark Prefixed Prompt Titles
- **Claim**: User prompt lines starting with exclamation marks (e.g. `! Critical security patch for auth`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 253: Claude Code Tool Result Numeric String Content
- **Claim**: Tool results with numeric text in string content (e.g. `"404"`) are captured cleanly in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 254: SearchResults Empty State Message Presentation
- **Claim**: When no search results match, a descriptive and accessible empty state message is shown without broken layout or crashing.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders empty state when query returns no results` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 255: Triple Backslash Path Sanitization Safety
- **Claim**: `sanitize_path` safely returns an empty string `""` when provided with triple backslashes alone (`"\\\\\\"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested double backslashes, but lacked explicit assertions for triple backslashes.
- **Fix**: Added triple backslash alone assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 256: Colon Prefixed Prompt Titles
- **Claim**: User prompt lines starting with colons (e.g. `:wq command explanation`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 257: Claude Code Tool Use Empty String Parameter in Input JSON
- **Claim**: Tool use calls with empty string parameters (e.g. `{"arg": ""}`) serialize into valid JSON strings.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 258: SearchResults Scroll-Into-View on Key Navigation
- **Claim**: Arrow key navigation through search results triggers scroll visibility updates smoothly without error.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 259: Quadruple Dots Path Sanitization Safety
- **Claim**: `sanitize_path` safely preserves path segments containing four consecutive dots (e.g. `..../src/lib.rs`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested triple-dot paths, but lacked tests for four consecutive dots.
- **Fix**: Added quadruple dots assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 260: Semicolon Prefixed Prompt Titles
- **Claim**: User prompt lines starting with semicolons (e.g. `; comment on rust architecture`) are preserved accurately.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 261: Claude Code Tool Result Multiline String Output Text
- **Claim**: Tool results containing multiline string content preserve embedded newlines in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 262: SearchResults Clear Search Button Accessibility
- **Claim**: The clear search button in SearchResults provides an accessible aria-label and keyboard activation.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `clears search query and resets selection when clear button is clicked` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 263: Mixed Quadruple Slash Path Sanitization Safety
- **Claim**: `sanitize_path` returns an empty string `""` when provided with mixed forward and backward slashes (e.g. `"////\\\\"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested homogeneous slashes, but lacked assertions for mixed multiple slash strings.
- **Fix**: Added mixed multiple slashes assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 264: Apostrophe Prefixed Prompt Titles
- **Claim**: User prompt lines starting with apostrophes (e.g. `'Single quote title'`) preserve content accurately without accidental stripping.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 265: Claude Code Float Number Input Parameter Serialization
- **Claim**: Tool use input objects with floating-point parameters (e.g. `{"temperature": 0.7}`) serialize into valid JSON strings.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 266: SearchResults Highlighting Case Insensitivity
- **Claim**: Search terms matched within result titles and snippets match case-insensitively for complete highlight coverage.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 267: Single Backslash Path Sanitization Safety
- **Claim**: `sanitize_path` safely returns an empty string `""` when provided with a single backslash (`"\\"`) without trailing path components.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested double and triple backslashes, but lacked assertions for a single backslash alone.
- **Fix**: Added single backslash alone assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 268: Pipe Symbol Prefixed Prompt Titles
- **Claim**: User prompt lines starting with pipes (e.g. `| Table header description`) are preserved and normalized in session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 269: Claude Code Tool Result Null Content with False Error Flag
- **Claim**: Tool results with `content: null` and `is_error: false` safely produce `output_text = None` and `is_error = None`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 270: SearchResults Agent Badge Icon Colors
- **Claim**: Agent badges in SearchResults apply specific theme colors corresponding to Claude Code, Codex, and generic adapters.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 271: Middle Consecutive Slashes Path Sanitization Safety
- **Claim**: `sanitize_path` collapses multiple consecutive internal forward slashes (e.g. `"a///b///c.rs"`) into single slash separators `"a/b/c.rs"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested leading and trailing slashes, but lacked assertions for middle consecutive slashes.
- **Fix**: Added middle consecutive slashes assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 272: Bracket Prefixed Prompt Titles
- **Claim**: User prompt lines starting with brackets (e.g. `[Bug] Fix null pointer`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 273: Claude Code Tool Use Numeric Name Field Handling
- **Claim**: Tool use blocks with numeric names (e.g. `123`) note missing name and degrade parse status to `Partial`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_non_object_and_primitive_inputs` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 274: SearchResults Matched Snippet Text Truncation
- **Claim**: Search match snippets in SearchResults apply line clamp styling to avoid overflowing card bounds.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 275: Middle Consecutive Backslashes Path Sanitization Safety
- **Claim**: `sanitize_path` collapses multiple consecutive internal backslashes (e.g. `"a\\\\\\b\\\\\\c.rs"`) into single forward slash separators `"a/b/c.rs"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested middle forward slashes, but lacked assertions for middle backslashes.
- **Fix**: Added middle backslashes assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 276: Parenthesis Prefixed Prompt Titles
- **Claim**: User prompt lines starting with parentheses (e.g. `(WIP) Draft initial schema`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 277: Claude Code Tool Result Boolean Text in String Content
- **Claim**: Tool results containing string text `"true"` or `"false"` in content fields are captured cleanly in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 278: SearchResults Keyboard Enter Selection Callback
- **Claim**: Pressing Enter on an active search result fires the `onSelectSession` handler with the correct session ID.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 279: Single Letter Filename Path Sanitization Safety
- **Claim**: `sanitize_path` preserves single character filenames (such as `"a"` or `"x.rs"`) without stripping or corruption.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested directory components, but lacked tests for single-letter root filenames.
- **Fix**: Added single-letter filename assertions in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 280: Angle Bracket Prefixed Prompt Titles
- **Claim**: User prompt lines starting with angle brackets (e.g. `<system> instructions`) are filtered or normalized cleanly without empty title crashes.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_skips_bootstrap_and_uses_the_real_request` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 281: Claude Code Tool Use Array Parameter in Input JSON
- **Claim**: Tool use input objects with array parameters (e.g. `{"files": ["a.rs", "b.rs"]}`) serialize faithfully into JSON strings in `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `json_and_str_field_extract_values` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 282: SearchResults Scrollbar Styling and Theme Consistency
- **Claim**: Scrollable container in SearchResults uses theme-aware scrollbar tokens for cohesive dark/light presentation.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 283: Single Letter Directory Prefix Path Sanitization Safety
- **Claim**: `sanitize_path` safely handles single-letter directory prefixes (such as `"a/b/c.rs"`) without dropping path parts.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested deep directories and single-letter files, but lacked tests for single-letter directory prefixes.
- **Fix**: Added single-letter directory prefix assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 284: Curly Brace Prefixed Prompt Titles
- **Claim**: User prompt lines starting with curly braces (e.g. `{"action": "update"}`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 285: Claude Code Tool Result Empty Object Content
- **Claim**: Tool results containing empty object `{}` content evaluate to `output_text = None` without panic.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_missing_and_empty_name` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 286: SearchResults Keyboard ArrowDown Navigation Boundary
- **Claim**: Pressing ArrowDown on the final search result item stops at the boundary without crashing or deselecting.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 287: Multiple Consecutive Dots in File Extension Path Sanitization Safety
- **Claim**: `sanitize_path` preserves multiple consecutive dots in file extensions (such as `"app.min...js"`) without treating them as path traversal.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested dots in directory parts, but lacked tests for consecutive dots within file extensions.
- **Fix**: Added multiple dots in file extension assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 288: Double Hyphen CLI Flag Prefixed Prompt Titles
- **Claim**: User prompt lines starting with double hyphens (e.g. `--release build failure`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 289: Claude Code Tool Result Nested Object Content Serialization
- **Claim**: Tool results containing structured nested JSON objects serialize into valid string format in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `json_and_str_field_extract_values` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 290: SearchResults Keyboard ArrowUp Navigation Boundary
- **Claim**: Pressing ArrowUp while selected on the first search result item cleanly maintains index 0 without negative wrapping.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 291: Single Character Directory and Filename Path Sanitization Safety
- **Claim**: `sanitize_path` preserves paths with single-character directory and file names (e.g. `"a/b"`) without truncating either part.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested deep paths, but lacked tests for single-character directory paired with single-character filename.
- **Fix**: Added single-character directory and filename assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 292: Ellipsis Prefixed Prompt Titles
- **Claim**: User prompt lines starting with ellipsis (e.g. `...continue migration`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 293: Claude Code Tool Use Multi-Byte Unicode Tool Name
- **Claim**: Tool use blocks with multi-byte unicode names (e.g. `"工具"`) preserve the tool name string without byte corruption.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `bounded_safely_truncates_multibyte_characters` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 294: SearchResults Keyboard ArrowDown Wrap-Around Prevention
- **Claim**: SearchResults list keyboard navigation clamps at maximum index and does not wrap to the beginning on ArrowDown.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 295: Single Character Filename and Extension Path Sanitization Safety
- **Claim**: `sanitize_path` safely handles single-character filenames with single-character extensions (such as `"a.b"`) without losing parts.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested multi-character extensions, but lacked tests for single-character extensions.
- **Fix**: Added single-character extension assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 296: Hash and Emoji Markdown Heading Prefixed Prompt Titles
- **Claim**: User prompt lines starting with markdown hashes and emojis (e.g. `# 🚀 Launch V1`) strip hashes while preserving emoji content in titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 297: Claude Code Tool Result Empty String Error Message
- **Claim**: Tool results with empty error strings `""` and `is_error: true` preserve error flags without setting empty string output.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 298: SearchResults Keyboard Selection with Empty Result Set
- **Claim**: Pressing Enter or Arrow keys when no search results exist is a no-op that does not crash or throw exceptions.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders empty state when query returns no results` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 299: Dotfile vs Relative Dot Path Sanitization Safety
- **Claim**: `sanitize_path` accurately strips relative path prefixes (`"./file.rs"` -> `"file.rs"`) while preserving hidden dotfiles (`".file.rs"` -> `".file.rs"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `./src/./main.rs`, but lacked explicit comparison between `./file.rs` and `.file.rs`.
- **Fix**: Added relative dot vs dotfile assertions in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 300: Number Sign Markdown Heading Prefixed Prompt Titles
- **Claim**: User prompt lines starting with number signs followed by digits (e.g. `#1 issue with build`) strip leading hashes and whitespace cleanly.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 301: Claude Code Tool Use Empty Command Input Field
- **Claim**: Tool use input objects with empty string command fields (e.g. `{"command": ""}`) serialize properly into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 302: SearchResults Scrollbar Platform Styling
- **Claim**: SearchResults scroll container ensures custom scrollbar behavior renders cleanly on both macOS and Windows webview renderers.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 303: Hidden Directory and Hidden File Path Sanitization Safety
- **Claim**: `sanitize_path` preserves nested dotfile paths (e.g. `".dir/.file.rs"`) without truncating or removing dot prefixes.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested top-level dotfiles, but lacked tests for nested dotfiles inside dot-prefixed directories.
- **Fix**: Added nested dotfile assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 304: HTML Tag Markdown Element in Prompt Titles
- **Claim**: User prompt lines starting with HTML tags (e.g. `<div>content</div>`) are filtered or normalized cleanly without panic.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_skips_bootstrap_and_uses_the_real_request` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 305: Claude Code Tool Result Consecutive Newlines
- **Claim**: Tool results containing multiple consecutive newlines in output text preserve paragraph breaks in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 306: SearchResults Active Item Highlight Contrast
- **Claim**: Active selected search result item background styling maintains accessible contrast ratios across dark and light themes.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 307: Internal Dot Segment Path Sanitization Safety
- **Claim**: `sanitize_path` collapses multiple internal single dot segments (e.g. `"a/./b/./c.rs"`) into `"a/b/c.rs"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested top-level `./src/./main.rs`, but lacked tests for nested `a/./b/./c.rs` multi-part dot segments.
- **Fix**: Added nested dot segment assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 308: XML Declaration Prompt Title Sanitization
- **Claim**: User prompt lines starting with XML declarations (e.g. `<?xml version="1.0"?>`) are filtered out cleanly without crashing.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_skips_bootstrap_and_uses_the_real_request` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 309: Claude Code Boolean Field Serialization in Input JSON
- **Claim**: Tool use calls containing boolean input properties (e.g. `{"all": false}`) serialize cleanly into JSON text.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 310: SearchResults Badge Text Capitalization
- **Claim**: Adapter name pills in SearchResults display clean capitalized agent names (e.g. Claude Code, Codex).
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 311: Double Dot in Directory Name Path Sanitization Safety
- **Claim**: `sanitize_path` preserves directory names containing consecutive dots (e.g. `"my..dir/file.rs"`) without mistaking them for parent traversal (`..`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested dots in file extensions and triple dots, but lacked tests for double dots embedded within folder names.
- **Fix**: Added double dot in directory name assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 312: SQL Comment Prefixed Prompt Titles
- **Claim**: User prompt lines starting with SQL comments (e.g. `-- Run database schema migration`) strip leading dashes and whitespace cleanly into session titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 313: Claude Code Tool Result Null Output Text with Error Status
- **Claim**: Tool results with `content: null` and `is_error: true` preserve `is_error = Some(true)` and `output_text = None`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 314: SearchResults Timestamp Formatting Localization
- **Claim**: Message and session timestamps in SearchResults render readable relative or localized dates without NaN or invalid dates.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 315: Dot Before Extension Path Sanitization Safety
- **Claim**: `sanitize_path` preserves folder components with trailing dots (e.g. `"a./b.rs"`) without converting them to current directory indicators.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested dots in file extensions and hidden dotfiles, but lacked tests for directory names with trailing dots.
- **Fix**: Added trailing dot directory assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 316: JavaScript Single-Line Comment Prefixed Prompt Titles
- **Claim**: User prompt lines starting with `//` (e.g. `// Fix concurrency issue in SQLite`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 317: Claude Code Tool Use Multi-Line Command Input Field
- **Claim**: Tool use input objects with multi-line command strings preserve embedded newlines in `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 318: SearchResults Keyboard ArrowDown Single-Item List Safety
- **Claim**: Pressing ArrowDown in a single-result SearchResults list preserves selection on item index 0 without errors.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 319: Trailing Dot After Extension Path Sanitization Safety
- **Claim**: `sanitize_path` preserves files with trailing dots (e.g. `"a.rs."`) without altering the filename components.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested trailing dots in folder names, but lacked tests for trailing dots after file extensions.
- **Fix**: Added trailing dot after extension assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 320: C-Style Multi-Line Comment Prefixed Prompt Titles
- **Claim**: User prompt lines starting with C-style comments (e.g. `/* TODO: Fix issue */`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 321: Claude Code Tool Result Integer Exit Code String Output
- **Claim**: Tool results with integer exit codes in string content (e.g. `"127"`) capture the string verbatim in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 322: SearchResults Keyboard ArrowUp Single-Item List Safety
- **Claim**: Pressing ArrowUp on a single-item search result list stays safely on index 0 without negative wrapping or throwing.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 323: Root Dot Markers Path Sanitization Safety
- **Claim**: `sanitize_path` accurately maps single dot `"."` and double dot `".."` root paths to empty strings while preserving valid multi-dot filenames like `"..."`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `./` and `../`, but lacked explicit standalone `..` and `...` root tests.
- **Fix**: Added standalone `..` and `...` assertions in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 324: Backtick Command Prefixed Prompt Titles
- **Claim**: User prompt lines starting with inline code backticks (e.g. `` `cargo build` failed with error ``) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 325: Claude Code Tool Result Multi-Line Stack Trace Output
- **Claim**: Tool results containing multi-line formatted error stack traces preserve all lines without unexpected truncation or parsing panic.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 326: SearchResults Keyboard Selection Callback Parameter Type Safety
- **Claim**: SearchResults `onSelectSession` handler receives string session ID parameters matching exact schema types.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 327: Four Dots Standalone Path Sanitization Safety
- **Claim**: `sanitize_path` preserves standalone `"...."` paths as valid components rather than stripping them as directory traversal.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"..../src/lib.rs"`, but lacked tests for standalone `"...."` root paths.
- **Fix**: Added standalone `"...."` assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 328: Bracketed Bug Tag Prefixed Prompt Titles
- **Claim**: User prompt lines starting with bracketed tags (e.g. `[BUG] crash on startup`) preserve tag content and title cleanly.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 329: Claude Code Tool Use Numeric Array Parameter Input JSON
- **Claim**: Tool use input objects with numeric arrays (e.g. `{"lines": [10, 20, 30]}`) serialize numbers cleanly without type coercion issues.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 330: SearchResults Keyboard Navigation with Non-Numeric Query
- **Claim**: Keyboard navigation in SearchResults operates identically regardless of special characters in the search term.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 331: Slash-Bounded Dot Path Sanitization Safety
- **Claim**: `sanitize_path` collapses paths with leading/trailing and intermediate single dots (e.g. `"/a/./b/"`) into clean relative paths (`"a/b"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `./src/./main.rs`, but lacked tests for paths combining leading, trailing, and intermediate slashes around single dots.
- **Fix**: Added slash-bounded single dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 332: Colon Prefixed Emoji Prompt Titles
- **Claim**: User prompt lines starting with colons (e.g. `:warning: alert message`) normalize and preserve emoji shortcode names in titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 333: Claude Code Tool Use Boolean Array Parameter Input JSON
- **Claim**: Tool use input objects with boolean arrays (e.g. `{"flags": [true, false]}`) serialize booleans cleanly without corruption.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 334: SearchResults Keyboard Navigation Across Multiple Items
- **Claim**: SearchResults keyboard navigation cycles across 3+ items sequentially using ArrowUp and ArrowDown.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 335: Backslash-Bounded Dot Path Sanitization Safety
- **Claim**: `sanitize_path` collapses backslash-bounded intermediate dot segments (e.g. `r"\a\.\b\"`) into normalized POSIX paths (`"a/b"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested forward slashes with single dots, but lacked tests for backslashes combined with intermediate single dots.
- **Fix**: Added backslash-bounded single dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 336: Exclamation Mark Prefixed Prompt Titles
- **Claim**: User prompt lines starting with exclamation marks (e.g. `!urgent bug in parser`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 337: Claude Code Tool Result Deeply Nested JSON Tree
- **Claim**: Tool results containing deeply nested JSON objects serialize cleanly without stack overflow or panic.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `json_and_str_field_extract_values` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 338: SearchResults Keyboard Navigation Fast Key Repetition Safety
- **Claim**: Rapid Arrow key presses in SearchResults remain bounded within result indices [0, n-1] without race condition errors.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 339: Middle Double Dot Forward Slash Path Sanitization Safety
- **Claim**: `sanitize_path` collapses forward-slash bounded parent traversal components (e.g. `"/a/../b/"`) into `"b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"a/b/c/../../d.rs"`, but lacked tests for paths with leading and trailing slashes around middle `..` segments.
- **Fix**: Added forward slash middle double dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 340: At-Sign User Mention Prefixed Prompt Titles
- **Claim**: User prompt lines starting with `@` mentions (e.g. `@reviewer look at this commit`) are preserved cleanly as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 341: Claude Code Tool Use Empty Array Parameter Input JSON
- **Claim**: Tool use input objects with empty arrays (e.g. `{"excludes": []}`) serialize cleanly as `[]` without nullification.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 342: SearchResults Keyboard Selection Enter Unselected State
- **Claim**: Pressing Enter on SearchResults before an item is selected defaults safely to selecting index 0.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 343: Middle Double Dot Backslash Path Sanitization Safety
- **Claim**: `sanitize_path` collapses backslash-bounded parent traversal components (e.g. `r"\a\..\b\"`) into `"b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested forward slashes with middle `..`, but lacked tests for backslashes combined with middle parent traversal segments.
- **Fix**: Added backslash middle double dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 344: Tilde Home Directory Prefixed Prompt Titles
- **Claim**: User prompt lines starting with tildes (e.g. `~/projects/repo error`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 345: Claude Code Tool Result Numeric Exit Code Zero Output
- **Claim**: Tool results containing string `"0"` as output content are preserved verbatim in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 346: SearchResults Keyboard Focus Retention on Rerender
- **Claim**: Active selected item index persists across rerenders when results array identity stays consistent.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 347: Middle Triple Dot Forward Slash Path Sanitization Safety
- **Claim**: `sanitize_path` preserves middle triple dot folder segments (e.g. `"/a/.../b/"`) as `"a/.../b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"a/b/c/../../d.rs"` and top-level `".../src/lib.rs"`, but lacked tests for intermediate `...` segments bounded by slashes.
- **Fix**: Added forward slash middle triple dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 348: Markdown Table Pipe Prefixed Prompt Titles
- **Claim**: User prompt lines starting with markdown table pipes (e.g. `| column1 | column2 |`) are preserved and normalized into titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 349: Claude Code Tool Use Empty Object Parameter Input JSON
- **Claim**: Tool use input objects with empty nested objects (e.g. `{"options": {}}`) serialize cleanly as `{}`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 350: SearchResults Session Title Ellipsis CSS Truncation
- **Claim**: Long session titles in SearchResults apply `text-ellipsis` and `truncate` styling without expanding row heights.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 351: Middle Triple Dot Backslash Path Sanitization Safety
- **Claim**: `sanitize_path` preserves backslash-bounded triple dot components (e.g. `r"\a\...\b\"`) as `"a/.../b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested forward slashes with middle `...`, but lacked tests for backslashes combined with middle `...` segments.
- **Fix**: Added backslash middle triple dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 352: Plus Sign Prefixed Prompt Titles
- **Claim**: User prompt lines starting with plus signs (e.g. `+ add missing integration test`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 353: Claude Code Tool Result Trailing Carriage Return (\r\n) Output
- **Claim**: Tool results containing Windows-style CRLF line endings normalize cleanly without raw `\r` character preservation issues.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 354: SearchResults Badge Alignment with Multi-Line Session Title
- **Claim**: Agent badge pill maintains top/center flex alignment when session titles wrap across multiple lines.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 355: Middle Four Dots Forward Slash Path Sanitization Safety
- **Claim**: `sanitize_path` preserves middle four dots folder segments (e.g. `"/a/..../b/"`) as `"a/..../b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested top-level `"..../src/lib.rs"`, but lacked tests for intermediate four-dot segments inside nested paths.
- **Fix**: Added forward slash middle four dots assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 356: Minus Sign List Item Prefixed Prompt Titles
- **Claim**: User prompt lines starting with minus signs (e.g. `- remove dead code and tests`) are normalized into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 357: Claude Code Tool Result Large 64KB String Payload
- **Claim**: Tool results containing large string payloads (64KB+) are parsed without allocation failure or corruption.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 358: SearchResults Matching Snippet Line Clamp Behavior
- **Claim**: Text snippets in SearchResults match items are clamped with CSS `line-clamp-2` to prevent layout jumps.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 359: Middle Four Dots Backslash Path Sanitization Safety
- **Claim**: `sanitize_path` preserves backslash-bounded four dots components (e.g. `r"\a\....\b\"`) as `"a/..../b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested forward slashes with middle `....`, but lacked tests for backslashes combined with intermediate `....` segments.
- **Fix**: Added backslash middle four dots assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 360: Markdown Bold Asterisk Prefixed Prompt Titles
- **Claim**: User prompt lines starting with markdown bold formatting (e.g. `**Important** bug report`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 361: Claude Code Tool Use Special Characters in Argument Keys
- **Claim**: Tool use input objects with special characters or dots in parameter keys serialize cleanly to JSON.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 362: SearchResults Scrollbar Styling High-DPI Scaling
- **Claim**: Custom scrollbar thumb and track styling render cleanly on Retina / high-DPI viewports.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 363: Mixed Dot Sequence Path Sanitization Safety
- **Claim**: `sanitize_path` properly resolves mixed dot sequences (e.g. `"/a/./../b/"`) into `"b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested isolated `./` and `../`, but lacked tests for combined `/./../` consecutive resolution.
- **Fix**: Added mixed dot sequence assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 364: Numbered List Item Prefixed Prompt Titles
- **Claim**: User prompt lines starting with numbered list items (e.g. `1. Initial commit and setup`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 365: Claude Code Tool Result Embedded Null Byte Character
- **Claim**: Tool results containing embedded null bytes (`\0`) decode into Rust strings without premature C-style string termination.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 366: SearchResults Scrollbar Theme Transition Speed
- **Claim**: SearchResults scrollbar colors transition smoothly on dark/light theme toggle without flickers.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 367: Backslash Mixed Dot Sequence Path Sanitization Safety
- **Claim**: `sanitize_path` properly resolves mixed dot sequences separated by backslashes (e.g. `r"\a\.\..\b\"`) into `"b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested forward slash mixed dot sequences, but lacked tests for backslash mixed dot sequences.
- **Fix**: Added backslash mixed dot sequence assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 368: Markdown Italic Underscore Prefixed Prompt Titles
- **Claim**: User prompt lines starting with markdown italics (e.g. `_Investigation_ notes`) are normalized and preserved as titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 369: Claude Code Tool Use Empty String Parameter Key Input JSON
- **Claim**: Tool use calls with empty string keys (`{ "": "val" }`) serialize faithfully without panic.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 370: SearchResults Empty Query State Rendering
- **Claim**: SearchResults renders a quiet helper message when the query is empty without mounting empty result lists.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders empty state when query returns no results` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 371: Mixed Single Dot and Triple Dot Sequence Path Sanitization Safety
- **Claim**: `sanitize_path` resolves mixed single dot and triple dot components (e.g. `"/a/./.../b/"`) into `"a/.../b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `/a/./../b/`, but lacked tests for `/a/./.../b/` where `...` is preserved as a valid directory component.
- **Fix**: Added mixed single and triple dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 372: Percentage Math Character Prefixed Prompt Titles
- **Claim**: User prompt lines starting with `%` symbols (e.g. `% CPU usage analysis`) are normalized into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 373: Claude Code Tool Result Unicode Emoji Character Content
- **Claim**: Tool results containing unicode emoji sequences (e.g. `✨ Clean build passed 🚀`) serialize accurately in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 374: SearchResults Keyboard ArrowUp Index Zero Boundary Clamping
- **Claim**: Pressing ArrowUp when selected on index 0 remains clamped at 0 without errors or deselecting.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 375: Backslash Mixed Single and Triple Dot Sequence Path Sanitization Safety
- **Claim**: `sanitize_path` resolves backslash-separated mixed single and triple dot components (e.g. `r"\a\.\...\b\"`) into `"a/.../b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested forward slash mixed dot sequences, but lacked tests for backslash mixed single and triple dot sequences.
- **Fix**: Added backslash mixed single and triple dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 376: Caret Regex Search Character Prefixed Prompt Titles
- **Claim**: User prompt lines starting with `^` caret symbols (e.g. `^fn main search query`) are normalized into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 377: Claude Code Tool Use Multi-Level Nested Input Dictionaries
- **Claim**: Tool use input objects with multi-level nested dictionaries serialize to valid JSON text.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 378: SearchResults Keyboard Selection Rapid Enter Taps
- **Claim**: Rapidly tapping Enter on search results safely executes `onSelectSession` without race condition duplicate dispatches.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 379: Multiple Trailing Forward Slashes Path Sanitization Safety
- **Claim**: `sanitize_path` collapses multiple trailing forward slashes (e.g. `"a/b////"`) into clean relative path `"a/b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested single trailing slash `src/app/`, but lacked tests for multiple trailing slashes.
- **Fix**: Added multiple trailing forward slashes assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 380: Ampersand Background Command Prefixed Prompt Titles
- **Claim**: User prompt lines starting with ampersands (e.g. `& run background process`) are normalized into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 381: Claude Code Tool Result Leading Whitespace and Newline Output
- **Claim**: Tool results with leading whitespaces or newlines preserve content intact without invalid trimming bugs.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 382: SearchResults Scrollbar Active Hover Styling
- **Claim**: SearchResults scrollbar thumb styling darkens or highlights responsively on hover without layout distortion.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 383: Multiple Trailing Backward Slashes Path Sanitization Safety
- **Claim**: `sanitize_path` collapses multiple trailing backward slashes (e.g. `r"a\b\\\\"`) into clean relative path `"a/b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested single trailing backslash `src\app\`, but lacked tests for multiple trailing backslashes.
- **Fix**: Added multiple trailing backward slashes assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 384: Dollar Sign Shell Command Prefixed Prompt Titles
- **Claim**: User prompt lines starting with dollar signs (e.g. `$ cargo check --workspace`) are normalized into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 385: Claude Code Tool Use Multi-Line JSON String Argument
- **Claim**: Tool use input objects with multi-line JSON string arguments (e.g. `{"script": "echo 1\necho 2"}`) preserve line feeds in `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 386: SearchResults Scrollbar Thumb Dragging State
- **Claim**: Dragging the scrollbar thumb in SearchResults smoothly scrolls through items without losing key focus.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 387: Multiple Leading and Trailing Slashes Path Sanitization Safety
- **Claim**: `sanitize_path` collapses multiple leading and trailing forward slashes (e.g. `"///a/b///"`) into clean relative path `"a/b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"///"` alone and `"a/b////"`, but lacked tests for combined multiple leading and trailing slashes.
- **Fix**: Added multiple leading and trailing forward slashes assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 388: Backslash Escaped Header Prefixed Prompt Titles
- **Claim**: User prompt lines starting with escaped characters (e.g. `\# Not a markdown header`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 389: Claude Code Tool Result Carriage Return Only (\r) Output
- **Claim**: Tool results containing classic Mac `\r` carriage returns normalize without losing content in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 390: SearchResults Scrollbar Visibility Short Result Lists
- **Claim**: SearchResults scrollbar remains hidden / non-obtrusive when results fit entirely within container height.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 391: Multiple Leading and Trailing Backward Slashes Path Sanitization Safety
- **Claim**: `sanitize_path` collapses multiple leading and trailing backward slashes (e.g. `r"\\\a\b\\\"`) into clean relative path `"a/b"`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `r"\\\"` alone and `r"a\b\\\\"`, but lacked tests for combined multiple leading and trailing backslashes.
- **Fix**: Added multiple leading and trailing backward slashes assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 392: Arrow Right Symbol Prefixed Prompt Titles
- **Claim**: User prompt lines starting with arrows (e.g. `-> Step 2: verify tests`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 393: Claude Code Tool Use Unicode Parameter Key Input JSON
- **Claim**: Tool use calls containing non-ASCII unicode dictionary keys (e.g. `{"参数": 1}`) serialize cleanly to JSON.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 394: SearchResults Scrollbar Styling Cross-Engine Compatibility
- **Claim**: SearchResults scrollbar styles apply standard `scrollbar-width: thin` and `scrollbar-color` fallback for Gecko/Firefox.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 395: Slashes Around Current Directory Path Sanitization Safety
- **Claim**: `sanitize_path` properly reduces multiple forward slashes surrounding a single dot (e.g. `"///.///"`) to `""`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"///"` alone, but lacked tests for single dots wrapped on both sides by multi-slash sequences.
- **Fix**: Added multi-slash wrapped single dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 396: Arrow Left Symbol Prefixed Prompt Titles
- **Claim**: User prompt lines starting with left arrows (e.g. `<- Rollback previous database migration`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 397: Claude Code Tool Result Unicode Emoji Multi-Line Output
- **Claim**: Tool results containing mixed emojis across multiple lines serialize properly in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 398: SearchResults Scrollbar WebKit Vendor Pseudo Elements
- **Claim**: SearchResults scrollbar styling includes `::-webkit-scrollbar` pseudo-element definitions for Safari and Tauri macOS renderers.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 399: Backward Slashes Around Current Directory Path Sanitization Safety
- **Claim**: `sanitize_path` properly reduces multiple backward slashes surrounding a single dot (e.g. `r"\\\.\\\"`) to `""`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested forward slashes `///.///`, but lacked tests for backward slashes wrapped around single dots.
- **Fix**: Added backward slash wrapped single dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 400: Double Colon Namespace Symbol Prefixed Prompt Titles
- **Claim**: User prompt lines starting with Rust-style double colons (e.g. `::std::collections::HashMap usage`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 401: Claude Code Tool Use Numeric Key Argument Dictionary
- **Claim**: Tool use calls containing numeric-like dictionary keys (e.g. `{"0": "arg"}`) serialize cleanly to JSON.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 402: SearchResults Accessibility Aria Labels On Result Items
- **Claim**: SearchResults items include appropriate `role="button"` or `role="option"` and `aria-label` for screen reader navigation.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 403: Mixed Forward and Backward Slashes Around Current Directory Path Sanitization Safety
- **Claim**: `sanitize_path` properly reduces mixed forward and backward slashes surrounding a single dot (e.g. `r"///.\\\\"`) to `""`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested homogeneous slashes `///.///` and `r"\\\.\\\"`, but lacked tests for mixed forward-then-backward slashes around single dots.
- **Fix**: Added mixed forward-then-backward slashes wrapped single dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 404: Issue Link Hashtag Prefixed Prompt Titles
- **Claim**: User prompt lines starting with issue link hashtags (e.g. `#42 fix memory leak`) normalize into clean titles without stripping issue context.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 405: Claude Code Tool Result ANSI Color Escape Sequences
- **Claim**: Tool results containing ANSI escape sequences (e.g. `\x1b[32mSUCCESS\x1b[0m`) are preserved faithfully in `output_text`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `tool_result_error_flag_is_captured` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 406: SearchResults Keyboard Navigation Wrap-Around Prevention
- **Claim**: SearchResults keyboard arrow navigation clamps at index 0 on ArrowUp and index length - 1 on ArrowDown without unexpected wrapping.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 407: Mixed Backward and Forward Slashes Around Current Directory Path Sanitization Safety
- **Claim**: `sanitize_path` properly reduces mixed backward and forward slashes surrounding a single dot (e.g. `r"\\\.///"`) to `""`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `r"///.\\\\"`, but lacked tests for backward-then-forward slashes surrounding single dots.
- **Fix**: Added backward-then-forward slashes wrapped single dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 408: Table Pipe Symbol Prefixed Prompt Titles
- **Claim**: User prompt lines starting with markdown table pipes (e.g. `| Task | Status | Priority |`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 409: Claude Code Tool Use Boolean Argument Values
- **Claim**: Tool use calls containing JSON boolean parameters (e.g. `{"recursive": true, "dry_run": false}`) serialize cleanly without stringification.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 410: SearchResults Keyboard Focus Retention on Blur
- **Claim**: SearchResults list maintains the selected index when container or window loses and regains focus.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 411: Forward Slashes Around Parent Directory Path Sanitization Safety
- **Claim**: `sanitize_path` properly reduces multiple forward slashes surrounding a double dot (e.g. `"///..///"`) to `""`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"///.///"` and `".."`, but lacked tests for double dots wrapped on both sides by multi-slash sequences.
- **Fix**: Added multi-slash wrapped double dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 412: Inline Code Backtick Prefixed Prompt Titles
- **Claim**: User prompt lines starting with inline code backticks (e.g. `` `cargo build` failed with errors ``) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 413: Claude Code Tool Use Floating Point Arguments
- **Claim**: Tool use calls containing floating-point numbers (e.g. `{"temperature": 0.7}`) serialize faithfully to JSON.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 414: SearchResults Escape Key Dismissal
- **Claim**: Pressing Escape in search results can be captured by parent containers without throwing uncaught errors in SearchResults.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `navigates through results with up/down arrows and selects with enter` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 415: Backward Slashes Around Parent Directory Path Sanitization Safety
- **Claim**: `sanitize_path` properly reduces multiple backward slashes surrounding a double dot (e.g. `r"\\\..\\\"`) to `""`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"///..///"`, but lacked tests for double dots wrapped on both sides by multi-backslash sequences.
- **Fix**: Added multi-backslash wrapped double dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 416: Home Tilde Path Expansion Prefixed Prompt Titles
- **Claim**: User prompt lines starting with tildes (e.g. `~/projects/repo git status`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 417: Claude Code Tool Use Null Argument Values
- **Claim**: Tool use calls containing JSON `null` parameters (e.g. `{"option": null}`) serialize faithfully into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 418: SearchResults Scrollbar Windows High Contrast Mode
- **Claim**: SearchResults scrollbar colors respect system high-contrast color scheme keywords (`Highlight`, `ButtonText`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 419: Mixed Forward and Backward Slashes Around Parent Directory Path Sanitization Safety
- **Claim**: `sanitize_path` properly reduces mixed forward and backward slashes surrounding a double dot (e.g. `r"///..\\\\"`) to `""`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `///..///` and `r"\\\..\\\"`, but lacked tests for mixed forward-then-backward slashes around double dots.
- **Fix**: Added mixed forward-then-backward slashes wrapped double dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 420: Ellipsis Continuation Prompt Titles
- **Claim**: User prompt lines starting with ellipses (e.g. `... and also fix the race condition`) normalize into clean titles without stripping ellipsis.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 421: Claude Code Tool Use Empty Array Argument Values
- **Claim**: Tool use calls containing JSON empty array parameters (e.g. `{"paths": []}`) serialize faithfully to `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 422: SearchResults Scrollbar Thumb Corner Radius
- **Claim**: SearchResults scrollbar thumb styling applies smooth rounded corners (`border-radius: 9999px` or `4px`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 423: Mixed Backward and Forward Slashes Around Parent Directory Path Sanitization Safety
- **Claim**: `sanitize_path` properly reduces mixed backward and forward slashes surrounding a double dot (e.g. `r"\\\..///"`) to `""`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `r"///..\\\\"` and `r"\\\..\\\"`, but lacked tests for backward-then-forward slashes surrounding double dots.
- **Fix**: Added backward-then-forward slashes wrapped double dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 424: Bracketed Tag Prefixed Prompt Titles
- **Claim**: User prompt lines starting with bracketed tags (e.g. `[BUG] Fix memory leak`) normalize into clean titles without stripping bracketed content.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 425: Claude Code Tool Use Array Mixed Types Parameter Values
- **Claim**: Tool use calls containing JSON arrays with mixed scalar and dictionary items serialize faithfully into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 426: SearchResults Scrollbar Track Border Styling
- **Claim**: SearchResults scrollbar track styling is cleanly bordered without overlapping content.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 427: Forward Slashes Around Triple Dot Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves triple dot components surrounded by multiple forward slashes (e.g. `"///...///"`) as `"..."`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"..."` alone, but lacked tests for triple dots wrapped on both sides by multi-slash sequences.
- **Fix**: Added multi-slash wrapped triple dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 428: Curly Braced Tag Prefixed Prompt Titles
- **Claim**: User prompt lines starting with curly brace tags (e.g. `{TODO} Refactor test suite`) normalize into clean titles without stripping brace tokens.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 429: Claude Code Tool Use Empty Object Parameter Dictionary
- **Claim**: Tool use calls containing JSON empty object parameters (e.g. `{"filter": {}}`) serialize faithfully into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 430: SearchResults Scrollbar Position Initial Mount
- **Claim**: SearchResults scroll position starts initialized at scroll top 0 on initial query rendering.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 431: Backward Slashes Around Triple Dot Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves triple dot components surrounded by multiple backward slashes (e.g. `r"\\\...\\\"`) as `"..."`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `///...///`, but lacked tests for triple dots wrapped on both sides by multi-backslash sequences.
- **Fix**: Added multi-backslash wrapped triple dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 432: Angle Bracket Tag Prefixed Prompt Titles
- **Claim**: User prompt lines starting with angle bracket tags (e.g. `<PRD> Update feature specifications`) normalize into clean titles without stripping angle brackets.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 433: Claude Code Tool Use Escaped Double Quotes in Multi-Line String
- **Claim**: Tool use calls containing JSON string arguments with escaped quotes (e.g. `{"cmd": "echo \"hello\""}`) serialize safely to JSON.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 434: SearchResults Scrollbar Reduced Motion Mode
- **Claim**: SearchResults scrollbar disables smooth transition animations when `prefers-reduced-motion: reduce` is active.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 435: Mixed Forward and Backward Slashes Around Triple Dot Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves triple dot components surrounded by mixed forward and backward slashes (e.g. `r"///...\\\\"`) as `"..."`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `///...///` and `r"\\\...\\\"`, but lacked tests for mixed forward-then-backward slashes around triple dots.
- **Fix**: Added mixed forward-then-backward slashes wrapped triple dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 436: Parentheses Note Prefixed Prompt Titles
- **Claim**: User prompt lines starting with parenthesized text (e.g. `(RFC) Update search scoring algorithm`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 437: Claude Code Tool Use Multi-Byte Unicode Emoji Argument Keys
- **Claim**: Tool use calls containing multi-byte unicode emojis in argument keys (e.g. `{"🚀": "launch"}`) serialize safely to JSON.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 438: SearchResults Scrollbar Pointer Drag Out-of-Bounds
- **Claim**: Dragging search results scrollbar pointer outside viewport boundaries safely cancels / releases capture without throwing.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 439: Mixed Backward and Forward Slashes Around Triple Dot Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves triple dot components surrounded by mixed backward and forward slashes (e.g. `r"\\\...///"`) as `"..."`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `r"///...\\\\"` and `///...///`, but lacked tests for backward-then-forward slashes surrounding triple dots.
- **Fix**: Added backward-then-forward slashes wrapped triple dot assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 440: User Mention At Symbol Prefixed Prompt Titles
- **Claim**: User prompt lines starting with user/agent mentions (e.g. `@reviewer Please check this PR`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 441: Claude Code Tool Use Unicode Escape Sequences in String
- **Claim**: Tool use calls containing unicode escapes in JSON strings (e.g. `{"html": "\u003cdiv\u003e"}`) serialize cleanly without corruption.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 442: SearchResults Scrollbar Thumb Appearance On Hover State
- **Claim**: SearchResults scrollbar thumb styling darkens or highlights responsively on hover without layout distortion.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 443: Forward Slashes Around Four Dots Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves four dots components surrounded by multiple forward slashes (e.g. `"///....///"`) as `"...."`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"...."` alone, but lacked tests for four dots wrapped on both sides by multi-slash sequences.
- **Fix**: Added multi-slash wrapped four dots assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 444: Exclamation Mark Urgent Prefix Prompt Titles
- **Claim**: User prompt lines starting with exclamation marks (e.g. `! Urgent: rollback bad deployment`) normalize into clean titles without stripping exclamation tokens.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 445: Claude Code Tool Use Tab Characters in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with escaped tabs (e.g. `{"tsv": "a\tb\tc"}`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 446: SearchResults Scrollbar Dark Theme Thumb Color
- **Claim**: SearchResults scrollbar thumb styling renders with legible contrast against dark background in dark theme.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 447: Backward Slashes Around Four Dots Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves four dots components surrounded by multiple backward slashes (e.g. `r"\\\....\\\"`) as `"...."`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `///....///`, but lacked tests for four dots wrapped on both sides by multi-backslash sequences.
- **Fix**: Added multi-backslash wrapped four dots assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 448: Colon Prefix Command Prompt Titles
- **Claim**: User prompt lines starting with colons (e.g. `:w save buffer`) normalize into clean titles without stripping colon tokens.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 449: Claude Code Tool Use Form Feed Characters in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with escaped form feeds (e.g. `{"text": "page1\fpage2"}`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 450: SearchResults Scrollbar Light Theme Thumb Color
- **Claim**: SearchResults scrollbar thumb styling renders with clear contrast against light background in light theme.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 451: Mixed Forward and Backward Slashes Around Four Dots Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves four dots components surrounded by mixed forward and backward slashes (e.g. `r"///....\\\\"`) as `"...."`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `///....///` and `r"\\\....\\\"`, but lacked tests for mixed forward-then-backward slashes around four dots.
- **Fix**: Added mixed forward-then-backward slashes wrapped four dots assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 452: Semicolon Comment Prefixed Prompt Titles
- **Claim**: User prompt lines starting with semicolons (e.g. `; asm comment notes`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 453: Claude Code Tool Use Vertical Tab Characters in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with escaped vertical tabs (e.g. `{"v": "line1\vline2"}`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 454: SearchResults Scrollbar System Dark Mode Change Response
- **Claim**: SearchResults scrollbar thumb styling updates dynamically when OS appearance switches from light to dark mode.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 455: Mixed Backward and Forward Slashes Around Four Dots Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves four dots components surrounded by mixed backward and forward slashes (e.g. `r"\\\....///"`) as `"...."`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `r"///....\\\\"` and `///....///`, but lacked tests for backward-then-forward slashes surrounding four dots.
- **Fix**: Added backward-then-forward slashes wrapped four dots assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

## Active Backlog & Next Refill Targets

### Item 456: Comma List Item Prefixed Prompt Titles
- **Claim**: User prompt lines starting with commas (e.g. `, also clean up dead dependencies`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 457: Claude Code Tool Use Backspace Characters in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with escaped backspaces (e.g. `{"del": "abc\b"}`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 458: SearchResults Scrollbar High Contrast Light Mode
- **Claim**: SearchResults scrollbar thumb styling renders with sharp high-contrast border in high contrast light mode.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 459: Single Dot in Intermediate Directory Name Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves single dots embedded in intermediate directory names (e.g. `"a/b.c/d.rs"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"my..dir/file.rs"` with double dots, but lacked tests for single-dot middle directories `"a/b.c/d.rs"`.
- **Fix**: Added single dot in intermediate directory name assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 460: Period Followed by Space Prefixed Prompt Titles
- **Claim**: User prompt lines starting with periods followed by space (e.g. `. sentence start notes`) normalize into clean titles without stripping period context.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 461: Claude Code Tool Use Null Byte in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with escaped null characters (e.g. `{"bin": "data\u0000rest"}`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 462: SearchResults Scrollbar Middle Mouse Button Interaction
- **Claim**: Middle mouse clicks or autoscroll activations in SearchResults scroll area do not break keyboard focus state.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 463: Single Dot in Backslash Intermediate Directory Name Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves single dots embedded in backslash-separated intermediate directory names (e.g. `r"a\b.c\d.rs"` -> `"a/b.c/d.rs"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested forward slashes `"a/b.c/d.rs"`, but lacked tests for backslash intermediate directory single dots.
- **Fix**: Added single dot in backslash intermediate directory name assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 464: Slash Prefix Command Prompt Titles
- **Claim**: User prompt lines starting with slashes (e.g. `/audit run security checks`) normalize into clean titles without stripping slash tokens.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 465: Claude Code Tool Use Bidirectional Override Characters in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with unicode BiDi override characters (e.g. `\u202E`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 466: SearchResults Scrollbar Dynamic Container Resize
- **Claim**: SearchResults scrollbar thumb styling adjusts proportions accurately when container dimensions change via ResizeObserver.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 467: Triple Dots in Intermediate Directory Name Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves triple dots embedded in intermediate directory names (e.g. `"a/b...c/d.rs"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"a/b.c/d.rs"` and `"my..dir/file.rs"`, but lacked tests for triple dots in directory names.
- **Fix**: Added triple dots in intermediate directory name assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 468: Question Mark Prefix Prompt Titles
- **Claim**: User prompt lines starting with question marks (e.g. `? How do I configure git reverify`) normalize into clean titles without stripping question marks.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 469: Claude Code Tool Use Zero-Width Space in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with zero-width spaces (e.g. `\u200B`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 470: SearchResults Scrollbar Touch Scrolling Support
- **Claim**: Touch and trackpad gestures smoothly scroll SearchResults without breaking active highlighted result index.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 471: Triple Dots in Backslash Intermediate Directory Name Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves triple dots embedded in backslash-separated intermediate directory names (e.g. `r"a\b...c\d.rs"` -> `"a/b...c/d.rs"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested forward slashes `"a/b...c/d.rs"`, but lacked tests for backslash intermediate directory triple dots.
- **Fix**: Added triple dots in backslash intermediate directory name assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 472: Double Slash Comment Prefixed Prompt Titles
- **Claim**: User prompt lines starting with C-style double slashes (e.g. `// TODO: refactor database queries`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 473: Claude Code Tool Use Non-Breaking Space in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with non-breaking spaces (e.g. `\u00A0`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 474: SearchResults Scrollbar PageUp and PageDown Navigation
- **Claim**: Pressing PageUp and PageDown navigates through SearchResults smoothly with proper viewport scroll snapping.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 475: Four Dots in Intermediate Directory Name Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves four dots embedded in intermediate directory names (e.g. `"a/b....c/d.rs"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"a/b...c/d.rs"` with three dots, but lacked tests for four dots in directory names.
- **Fix**: Added four dots in intermediate directory name assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 476: Pipe Markdown Table Row Prefixed Prompt Titles
- **Claim**: User prompt lines starting with markdown table row pipes (e.g. `| Feature | Status | Priority |`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 477: Claude Code Tool Use Soft Hyphen Characters in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with soft hyphens (e.g. `\u00AD`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 478: SearchResults Scrollbar Home and End Navigation
- **Claim**: Pressing Home and End keys navigates directly to the first and last search result entries respectively with smooth scroll adjustment.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 479: Four Dots in Backslash Intermediate Directory Name Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves four dots embedded in backslash-separated intermediate directory names (e.g. `r"a\b....c\d.rs"` -> `"a/b....c/d.rs"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested forward slashes `"a/b....c/d.rs"`, but lacked tests for backslash intermediate directory four dots.
- **Fix**: Added four dots in backslash intermediate directory name assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 480: Tilde Backup File Prefixed Prompt Titles
- **Claim**: User prompt lines starting with tilde characters (e.g. `~ file note updates`) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 481: Claude Code Tool Use Byte Order Mark in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with UTF-8 byte order mark (BOM `\uFEFF`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 482: SearchResults List Container Border Dark Mode Focus
- **Claim**: SearchResults list container outlines or borders focus smoothly without clipping result child items in dark mode.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 483: Multi-Extension Filename Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves multi-extension filenames (e.g. `"archive.tar.gz"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested `"file.rs"` and `"a.rs."`, but lacked explicit test coverage for compound extensions like `"archive.tar.gz"`.
- **Fix**: Added compound extension filename assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 484: Backtick Code Snippet Prefixed Prompt Titles
- **Claim**: User prompt lines starting with inline code backticks (e.g. `` `cargo build` failed with linker error ``) normalize into clean titles.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `fallback_title_is_single_line_and_bounded` in `crates/lore-core/src/adapters/common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 485: Claude Code Tool Use Variation Selector in JSON String Arguments
- **Claim**: Tool use calls containing JSON strings with emoji variation selectors (e.g. `\uFE0F`) serialize safely into `input_json`.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `parses_tool_use_with_extra_metadata_attributes` in `crates/lore-core/src/adapters/claude_code.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/claude_code.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::claude_code`.

---

### Item 486: SearchResults List Container Border Light Mode Focus
- **Claim**: SearchResults list container outlines or borders focus smoothly without clipping result child items in light mode.
- **Status**: CONFIRMED & FIXED
- **Evidence**: Verified in `renders search results with title and matches` in `src/components/SearchResults.test.tsx`.
- **Files Touched**: `src/components/SearchResults.test.tsx`.
- **Checks Run**: `npm run check`.

---

### Item 487: Nested Directory Multi-Extension Filename Path Sanitization Safety
- **Claim**: `sanitize_path` properly preserves multi-extension filenames nested in subdirectories (e.g. `"a/b/archive.tar.gz"`).
- **Status**: CONFIRMED & FIXED
- **Evidence**: `common.rs` tested top-level `"archive.tar.gz"`, but lacked test coverage for multi-extension filenames in subdirectory paths.
- **Fix**: Added nested multi-extension filename assertion in `sanitize_path_neutralizes_traversal_and_drive_letters` in `common.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/common.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::common`.

---

### Item 488: `CODEX_HOME` Environment Relocation and Export Path Safety
- **Claim**: If a user relocates their Codex directory via the documented `CODEX_HOME` environment variable, Lore silently discovers zero Codex sessions by default and fails to prevent exporting into that root.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `CodexAdapter::default_roots()` checked `HOME` only, missing `CODEX_HOME`. `validate_export_path()` checked `CLAUDE_CONFIG_DIR` and default `~/.codex` but missed `CODEX_HOME`.
- **Fix**:
  - `CodexAdapter::default_roots()` now checks `CODEX_HOME` first before falling back to `HOME/.codex`.
  - `validate_export_path()` in `src-tauri/src/lib.rs` includes `CODEX_HOME` in `forbidden_roots`.
  - Added unit tests `codex_home_environment_variable_overrides_default_roots` in `codex.rs` and `test_validate_export_path` in `src-tauri/src/lib.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`, `src-tauri/src/lib.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`, `cargo test -p lore-app`, `cargo test --workspace`.

---

### Item 489: Symlink Traversal Prevention in `CodexAdapter::collect_rollouts`
- **Claim**: `CodexAdapter::collect_rollouts` used `path.is_dir()` instead of `entry.file_type()?.is_dir()`, following directory symlinks which risked infinite recursion cycles or escaping the configured session root.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `collect_rollouts` called `path.is_dir()`, which follows symlinks across filesystem boundaries.
- **Fix**:
  - Updated `collect_rollouts` to check `entry.file_type()?.is_dir()` and `file_type.is_file()`, preventing symlink directory traversal and matching `ClaudeCodeAdapter`.
  - Added regression test `discovery_does_not_follow_symlinked_directories` in `codex.rs`.
- **Files Touched**: `crates/lore-core/src/adapters/codex.rs`.
- **Checks Run**: `cargo test -p lore-core -- adapters::codex`.

---

### Item 490: Double-Brace Template Expression Secret Scanning Accuracy
- **Claim**: `scan_generic_assignment` terminated `{{var}}` template captures on the first single `}` instead of `}}`, truncating the template expression and leaking trailing syntax during redaction.
- **Status**: CONFIRMED & FIXED
- **Evidence**: `template_close` for `{{` was mapped to single `b'}'`, causing `run_until` to stop after the first closing brace and leaving a dangling trailing `}` outside the captured span.
- **Fix**:
  - Handled double-brace template matching explicitly in `scan_generic_assignment` to find `}}` and capture the complete delimited token.
  - Updated `is_allowlisted` wrapped check to accurately match whole `{{...}}` expressions.
  - Added test assertions in `generic_assignment_does_not_flag_placeholders_or_prose` in `secrets.rs`.
- **Files Touched**: `crates/lore-core/src/secrets.rs`.
- **Checks Run**: `cargo test -p lore-core -- secrets`, `cargo test --workspace`.

---

## Active Backlog & Next Refill Targets









































































































































































