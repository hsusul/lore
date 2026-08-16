# POLISH_LOG.md — Continuous Polish and Improvement Loop

> Durable log of small, verified improvements across all lenses.
> Maintained autonomously on `polish-loop` branch.

## Iteration 1
- **Lens:** Correctness bugs
- **Change:** Support backslash (`\`) path separators and Windows-style traversal in `sanitize_path` (`crates/lore-core/src/adapters/common.rs`).
- **Critique:**
  - `sanitize_path` previously only split on forward slashes `/`.
  - Windows-style paths or agent tool arguments containing `\` with traversal (e.g. `..\..\etc\passwd`) were treated as single segments and not stripped.
  - Separators were not normalized to `/`.
  - Fix: Split on `['/', '\\']` in `sanitize_path` so traversal elements are popped and paths are cleanly normalized with `/`.
- **Validation Results:**
  - `cargo test --workspace`: 74 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Missing tests/edge cases:* `DiscoveryConfig::roots_for` returns default if empty list is passed or absent, but `owner_of` canonicalization behavior with trailing slashes or relative paths under custom roots can be edge-case tested.
  2. *Error handling:* `crates/lore-core/src/backup.rs` `restore_backup` removes destination DB on restore without atomic staging/rename or transactional fallback if subsequent integrity check fails.
  3. *Performance/allocations:* `crates/lore-core/src/query.rs` `keyset_after` constructs formatted strings and vectors for every keyset page; can optimize string allocations.

## Iteration 2
- **Lens:** Missing tests/edge cases
- **Change:** Harden folder `clean_name` normalization and add edge-case and keyset pagination tests for `list_folder_sessions_page` (`crates/lore-core/src/folders.rs`, `crates/lore-core/src/query.rs`).
- **Critique:**
  - `clean_name` in `folders.rs` previously preserved raw newlines/tabs inside names and could leave trailing whitespace after taking `MAX_NAME_LEN` (100) characters.
  - Keyset pagination for `list_folder_sessions_page` in `query.rs` lacked dedicated automated tests for multi-page traversals, cursor continuation, and folder isolation.
  - Fix: Normalized multi-line whitespace in `clean_name`, added edge-case tests, and added `folder_session_pages_stay_scoped_and_stable` test in `query.rs`.
- **Validation Results:**
  - `cargo test --workspace`: 75 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Error handling:* `crates/lore-core/src/backup.rs` `create_backup` / `restore_backup` directory creation errors and read-only destination handling can be made more informative and resilient.
  2. *Performance/allocations:* `crates/lore-core/src/secrets.rs` scanner regexes / patterns pass string slices that could reuse preallocated finding vectors.
  3. *UX & accessibility:* Command palette list items in `src/components/CommandPalette.tsx` keyboard selection aria attributes.

## Iteration 3
- **Lens:** Error handling
- **Change:** Clean up failed candidate backup files on verification failure in `create_backup` and handle non-existent directories gracefully in `list_backups` (`crates/lore-core/src/backup.rs`, `crates/lore-core/tests/backup.rs`).
- **Critique:**
  - `create_backup` previously left candidate backup files on disk if subsequent permission setting or integrity check failed, risking corrupted or unverified files remaining in `backups/`.
  - `list_backups` returned `Err(BackupError::Io)` when called on a not-yet-created backup directory rather than returning an empty list `Ok(vec![])`.
  - Fix: Self-cleaning error handling in `create_backup` removes candidate file on failure, `list_backups` returns `Ok(vec![])` if directory does not exist, and added test verification.
- **Validation Results:**
  - `cargo test --workspace`: 76 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Performance/allocations:* `crates/lore-core/src/secrets.rs` scanner patterns allocate new findings vectors on each scan; can optimize with capacity reservation.
  2. *API & DTO ergonomics:* Session summary DTO formatting or TypeScript bindings helper methods for displaying relative dates.
  3. *UX & accessibility:* Search results input and empty states accessibility tags in `src/components/SearchResults.tsx`.

## Iteration 4
- **Lens:** Performance/allocations
- **Change:** Eliminate intermediate heap string allocations and short-circuit empty findings in `secrets::redact` (`crates/lore-core/src/secrets.rs`).
- **Critique:**
  - `redact` previously cloned findings vector and created a new String with capacity even when `findings` was empty.
  - For each finding, `redact` called `format!("«redacted:{rule}»")`, allocating and dropping a temporary `String` on the heap for every masked span.
  - Fix: Short-circuit empty findings to `text.to_string()` directly, and stream `«redacted:`, `finding.rule`, and `»` directly into the preallocated buffer.
- **Validation Results:**
  - `cargo test --workspace`: 76 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *API & DTO ergonomics:* Command palette DTO / IPC query parameters error handling and TypeScript types validation.
  2. *Docs accuracy vs code:* Verify `docs/architecture/SECRET_SCANNING.md` matches current rule definitions and redaction behavior.
  3. *UX & accessibility:* Aria attributes and role properties for search result items in `src/components/SearchResults.tsx`.

## Iteration 5
- **Lens:** API & DTO ergonomics
- **Change:** Add default `cursor = null` parameter to `listFolderSessionsPage` and consolidate DTO type exports in `src/ipc.ts` (`src/ipc.ts`, `src/ipc.test.ts`).
- **Critique:**
  - `listFolderSessionsPage` in `src/ipc.ts` required callers to explicitly supply all 3 parameters, whereas all other paginated API functions (`listSessionsPage`, `listRepositorySessionsPage`, `searchPage`) provided default `= null` for `cursor`.
  - `BackupScheduleDto` was exported in a fragmented second `export type` statement rather than the primary consolidated DTO export block.
  - Fix: Added `cursor: string | null = null` to `listFolderSessionsPage`, consolidated type exports in `src/ipc.ts`, and added tests in `src/ipc.test.ts`.
- **Validation Results:**
  - `cargo test --workspace`: 76 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Docs accuracy vs code:* Check if `docs/architecture/ARCHITECTURE.md` §5 IPC table includes all newly added folder, backup, and settings IPC commands.
  2. *UX & accessibility:* Search results input and command palette list items aria attributes in `src/components/CommandPalette.tsx` and `src/components/SearchResults.tsx`.
  3. *Security/input validation:* Hostile Git configuration or invalid repo IDs in Git observation queries.

## Iteration 6
- **Lens:** Docs accuracy vs code
- **Change:** Document `Folder` and `SessionFolder` schema entities and indexes in `DATA_MODEL.md` and complete folder IPC commands in `ARCHITECTURE.md` §5 (`docs/architecture/DATA_MODEL.md`, `docs/architecture/ARCHITECTURE.md`).
- **Critique:**
  - `docs/architecture/DATA_MODEL.md` was missing documentation for `Folder` and `SessionFolder` tables introduced in migration 0008, violating the canonical data model synchronization rule.
  - `docs/architecture/ARCHITECTURE.md` §5 IPC commands table omitted the folder management commands (`list_folders`, `create_folder`, `rename_folder`, `delete_folder`, `set_session_folder`, `list_folder_sessions_page`).
  - Fix: Updated `DATA_MODEL.md` entity descriptions, relationship overview diagram, and index lists, and updated `ARCHITECTURE.md` §5 command list.
- **Validation Results:**
  - `cargo test --workspace`: 76 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *UX & accessibility:* Add ARIA attributes (`role="listbox"`, `role="option"`, `aria-selected`) to `src/components/CommandPalette.tsx` search results list.
  2. *Security/input validation:* Repository identity evidence validator edge cases.
  3. *Dead code & duplication:* Inactive or redundant utility functions in test fixtures.

## Iteration 7
- **Lens:** UX & accessibility
- **Change:** Add `role="status"` to empty search results announcement and polite status wrapper to search pagination in `SearchResults` (`src/components/SearchResults.tsx`, `src/components/SearchResults.test.tsx`).
- **Critique:**
  - In `src/components/SearchResults.tsx`, the zero-match search message lacked `role="status"`, preventing screen readers from announcing when a live search completed without matches (in contrast with the loading state which had `role="status"`).
  - The "Load more results" pagination control was not enclosed in a polite live region matching `SessionList`.
  - Fix: Added `role="status"` to the empty search message, normalized `aria-label` casing, wrapped pagination in a polite live status region, and added test verification.
- **Validation Results:**
  - `cargo test --workspace`: 76 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Security/input validation:* Hostile Git configuration or shell escape validation in git observation queries.
  2. *Dead code & duplication:* Inactive or redundant utility functions in adapter test helpers.
  3. *Naming/consistency:* Uniform error kind string constants across adapters and jobs.

## Iteration 8
- **Lens:** Security/input validation
- **Change:** Reject embedded control characters, null bytes, and internal whitespace in `normalize_remote_url` (`crates/lore-core/src/git.rs`).
- **Critique:**
  - `normalize_remote_url` trimmed external whitespace but allowed embedded newlines (`\r`, `\n`), null bytes (`\0`), or ASCII whitespace inside raw remote URL strings.
  - Malformed or poisoned agent session inputs could inject unescaped control characters into canonical normalized repository identity evidence and database fields (`git_observation.remote_url_norm`).
  - Fix: Checked `raw.chars().any(|c| c.is_ascii_control() || c.is_ascii_whitespace())` to immediately reject malformed URLs with `None`, and added regression tests.
- **Validation Results:**
  - `cargo test --workspace`: 76 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Dead code & duplication:* Inactive or redundant test helper functions across test suites.
  2. *Naming/consistency:* Consistent error message styling in storage recovery and backups.
  3. *ROADMAP progression:* Review V0 acceptance criteria status and verification coverage.

## Iteration 9
- **Lens:** Dead code & duplication
- **Change:** Centralize `str_field` JSON value extractor in `crates/lore-core/src/adapters/common.rs` and reuse across Claude Code and Codex adapters (`crates/lore-core/src/adapters/common.rs`, `crates/lore-core/src/adapters/claude_code.rs`, `crates/lore-core/src/adapters/codex.rs`).
- **Critique:**
  - `str_field` helper was duplicated verbatim across `claude_code.rs` and `codex.rs`.
  - Fix: Moved `str_field` to `adapters::common`, updated imports in both adapters, and removed duplicate private definitions.
- **Validation Results:**
  - `cargo test --workspace`: 76 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Naming/consistency:* Uniform error kinds in database quarantine / recovery operations.
  2. *ROADMAP progression:* Cross-check ROADMAP acceptance gate documentation with recent tests.
  3. *Dependency/build hygiene:* Unused dependencies or dev-dependencies in Cargo.toml manifests.

## Iteration 10
- **Lens:** Naming/consistency
- **Change:** Make `integrity_ok` return a direct boolean and treat unopenable SQLite files as not intact for automatic quarantine and recovery (`crates/lore-core/src/recovery.rs`, `crates/lore-core/tests/recovery.rs`).
- **Critique:**
  - `integrity_ok` returned `Result<bool>` and mapped `Connection::open` failures to `Err(RecoveryError::Io)`, causing severely corrupted databases that failed at open time to error out instead of evaluating as not intact (`false`).
  - As a consequence, unopenable corrupted archives failed `recover_archive` instead of being preserved under `quarantine/` and restored from backup.
  - Fix: Changed `integrity_ok` to return `bool` directly (returning `false` on any open or integrity check failure), removed unnecessary error propagation, and added regression test coverage.
- **Validation Results:**
  - `cargo test --workspace`: 77 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *ROADMAP progression:* Cross-check ROADMAP acceptance gate documentation with current test pass status.
  2. *Dependency/build hygiene:* Clean up any unused cargo profile flags or unnecessary dependencies.
  3. *Correctness bugs:* Verify keyset pagination edge case when all items have identical timestamps.

## Iteration 11
- **Lens:** ROADMAP progression
- **Change:** Reconcile `ROADMAP.md` milestone status with user folder management (M5) and backup schedule/settings UI (M7) (`docs/product/ROADMAP.md`).
- **Critique:**
  - `docs/product/ROADMAP.md` implementation status table and build sequence lagged behind merged capabilities, omitting user folders (`FolderList`, drag-and-drop thread filing, keyset pagination) and user-visible backup schedule UI (`BackupSettings`, interval and retention settings, manual trigger).
  - Fix: Updated `ROADMAP.md` M5 and M7 entries to reflect built capabilities accurately.
- **Validation Results:**
  - `cargo test --workspace`: 77 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Dependency/build hygiene:* Audit workspace Cargo.toml files for unused or outdated dependencies.
  2. *Correctness bugs:* Test pagination behavior when consecutive sessions share exact same millisecond timestamp.
  3. *Error handling:* Safe parsing of malformed JSON strings in `set_setting` IPC command.

## Iteration 12
- **Lens:** Dependency/build hygiene
- **Change:** Inherit workspace package metadata (`*.workspace = true`) in `src-tauri/Cargo.toml` (`src-tauri/Cargo.toml`).
- **Critique:**
  - `src-tauri/Cargo.toml` hardcoded duplicate `version`, `edition`, `rust-version`, `license`, `repository`, and `authors` fields instead of inheriting from `[workspace.package]` like the other workspace members (`lore-core`, `lore-ipc`).
  - Fix: Changed `src-tauri/Cargo.toml` package metadata to inherit workspace fields consistently.
- **Validation Results:**
  - `cargo test --workspace`: 77 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Correctness bugs:* Verify keyset pagination edge case when consecutive sessions have identical millisecond timestamps and ensure strict tie-breaking.
  2. *Missing tests/edge cases:* Query pagination with null timestamps boundary conditions.
  3. *Error handling:* Parse safety on malformed JSON payload strings in `set_setting` IPC command.

## Iteration 13
- **Lens:** Correctness bugs
- **Change:** Add deterministic regression test verifying keyset pagination tie-breaking over sessions sharing identical timestamps (`crates/lore-core/src/query.rs`).
- **Critique:**
  - Keyset pagination across varying timestamps was tested, but an explicit regression proof was missing for multi-page traversals through bursts of sessions sharing the exact same millisecond timestamp where the secondary `(started_at = ? AND id < ?)` tie-breaker must prevent duplicate or skipped items across page boundaries.
  - Fix: Added `session_pages_handle_identical_timestamps_without_skips_or_duplicates` verifying complete, deduplicated iteration across 6 timestamp-colliding sessions with page limit 2.
- **Validation Results:**
  - `cargo test --workspace`: 78 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Missing tests/edge cases:* Test `set_setting` and `get_setting` IPC commands with arbitrary JSON types (strings, booleans, objects).
  2. *Error handling:* Verify error feedback when `rename_folder` is called with a blank name or nonexistent ID.
  3. *Performance/allocations:* String allocation tuning in `query_session_page`.

## Iteration 14
- **Lens:** Missing tests/edge cases
- **Change:** Add edge-case unit tests for folder foreign-key enforcement on unknown session IDs and safe no-ops for nonexistent folder rename/delete (`crates/lore-core/src/folders.rs`).
- **Critique:**
  - `folders.rs` tested foreign-key errors when filing into an unknown folder, but lacked explicit assertions for filing an unknown session ID (testing `session_folder.session_id` FK enforcement) and verifying that `rename_folder` and `delete_folder` on nonexistent folder IDs succeed idempotently as safe no-ops without error.
  - Fix: Added `filing_an_unknown_session_is_rejected` and `rename_and_delete_nonexistent_folder_are_safe_noops` tests.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Error handling:* Validate JSON format in `set_setting` IPC command to reject invalid JSON early.
  2. *Performance/allocations:* Optimize SQL query construction in `list_sessions_page`.
  3. *API & DTO ergonomics:* Ensure consistent naming for query filter options.

## Iteration 15
- **Lens:** Error handling
- **Change:** Validate JSON syntax in `set_setting` IPC command before persisting to SQLite (`src-tauri/src/lib.rs`).
- **Critique:**
  - `set_setting` accepted arbitrary string payloads and inserted them directly into `setting.value_json` without checking if they parse as valid JSON.
  - Invalid JSON strings would persist to the DB and cause subsequent typed setting deserializers (`read_schedule`, `get_bool`) to fail silently or corrupt state.
  - Fix: Added `serde_json::from_str::<serde_json::Value>(&value_json)` validation returning an explicit error if the JSON is malformed.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *Performance/allocations:* Optimize SQL query string allocations in `list_sessions_page` and `list_folder_sessions_page`.
  2. *API & DTO ergonomics:* Verify UI error message toasts when settings or root updates fail.
  3. *Docs accuracy vs code:* Verify ADR-0005 egress test requirements match current codebase guards.

## Iteration 16
- **Lens:** Performance/allocations
- **Change:** Eliminate heap string allocations in keyset pagination SQL predicate generation (`crates/lore-core/src/query.rs`).
- **Critique:**
  - `keyset_after` invoked `format!()` on every keyset-paginated query to construct dynamic `String` values for static SQL predicates across 4 known branches.
  - Keyset pagination runs on every scroll and "load more" action in the UI.
  - Fix: Changed `keyset_after` to return static `&'static str` string literals, eliminating runtime format/heap string allocations on every page query.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (85 tests).
- **Backlog Candidates Noticed:**
  1. *API & DTO ergonomics:* Audit TypeScript IPC interface helper typing for `null` / `undefined` optionals.
  2. *Docs accuracy vs code:* Check documentation of `keyset_after` and cursor encoding in `docs/architecture/DATA_MODEL.md`.
  3. *UX & accessibility:* Verify keyboard navigation focus trap and ESC handling on folder creation dialog.

## Iteration 17
- **Lens:** API & DTO ergonomics
- **Change:** Add default parameter for `exportSessionMarkdown` and define strongly-typed `BackupInterval` union in frontend IPC contract (`src/ipc.ts`, `src/ipc.test.ts`).
- **Critique:**
  - `exportSessionMarkdown` documented that `includeSecrets` defaulted to `false`, but the parameter was required without a TypeScript default parameter value.
  - `setBackupSchedule` accepted an unconstrained `string` for `interval` rather than the canonical `"off" | "daily" | "weekly"` union.
  - Fix: Added `includeSecrets: boolean = false`, defined/exported `BackupInterval`, and added tests in `src/ipc.test.ts`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (87 tests).
- **Backlog Candidates Noticed:**
  1. *Docs accuracy vs code:* Verify documentation of ADR-0005 egress guard tests.
  2. *UX & accessibility:* Ensure focus returns to folder trigger after folder modal dismiss.
  3. *Security/input validation:* Check path sanitization in blob store read operations.

## Iteration 18
- **Lens:** Docs accuracy vs code
- **Change:** Document backup schedule configuration settings in canonical data model specification (`docs/architecture/DATA_MODEL.md`).
- **Critique:**
  - `docs/architecture/DATA_MODEL.md` §3 documented `agent_roots.<agent_id>` under `Setting`, but omitted the canonical `backup.interval`, `backup.keep`, and `backup.last_at` settings keys used by the local backup subsystem (`crates/lore-core/src/backup.rs`).
  - Fix: Documented backup schedule settings keys, valid interval values, and retention bounds in `DATA_MODEL.md` §3.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (87 tests).
- **Backlog Candidates Noticed:**
  1. *UX & accessibility:* Add aria-label and keyboard shortcut tips to folder filter actions.
  2. *Security/input validation:* Path traversal guards on blob relpath reads.
  3. *Dead code & duplication:* Consolidate any redundant test helpers across integration test files.

## Iteration 19
- **Lens:** UX & accessibility
- **Change:** Add F2 keyboard shortcut for inline folder rename and improve landmark/counter aria labels (`src/components/FolderList.tsx`, `src/components/FolderList.test.tsx`).
- **Critique:**
  - Folders could previously only be renamed via mouse double-click, locking out keyboard-only and assistive-technology users from renaming folders.
  - The navigation landmark had a lowercase `aria-label="folders"`, and the thread count badge lacked descriptive context for screen readers.
  - Fix: Added `onKeyDown` with `F2` key handler to trigger rename on focused folder, capitalized navigation label to `aria-label="Folders"`, added `aria-label="${folder.session_count} threads"` on count badge, and added test in `FolderList.test.tsx`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *Security/input validation:* Verify path normalization in `crates/lore-core/src/storage/blob.rs` for Windows backslash safety.
  2. *Dead code & duplication:* Consolidate temporary directory creation test fixtures.
  3. *Naming/consistency:* Audit error type naming across storage and backup modules.

## Iteration 20
- **Lens:** Security/input validation
- **Change:** Harden `safe_relpath` against Windows backslash traversal and drive letter prefixes in blob storage (`crates/lore-core/src/storage/blob.rs`).
- **Critique:**
  - `safe_relpath` in `blob.rs` previously split only on `'/'`. On Windows systems or with mixed path syntax, backslashes (`'\\'`) or drive-letter prefixes (such as `..\..\file` or `C:\file`) were not properly segmented, leaving potential directory traversal risk outside `blobs/`.
  - Fix: Updated `safe_relpath` to split on `['/', '\\']` and reject any empty segment, `..`, `.`, or `:` drive prefix, with full regression assertions.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *Dead code & duplication:* Consolidate `test_dir` / temp store creation across core integration test suites.
  2. *Naming/consistency:* Ensure uniform test fixture naming across parser modules.
  3. *ROADMAP progression:* Audit milestone tracker for M6/M7 verification notes.

## Iteration 21
- **Lens:** Dead code & duplication / schema coverage
- **Change:** Add folder table and index assertions to schema regression suite (`crates/lore-core/tests/schema.rs`).
- **Critique:**
  - `crates/lore-core/tests/schema.rs` verified tables and performance indexes from migrations 0001..0005, but had not been updated to assert the existence of the `folder` and `session_folder` tables or the `ix_session_folder_folder` index introduced in migration 0008.
  - Fix: Added `"folder"` and `"session_folder"` to `all_v0_tables_exist` and `"ix_session_folder_folder"` to `data_model_indexes_exist`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *Naming/consistency:* Reconcile error enum message formats across `SourceRootError` and `BackupError`.
  2. *ROADMAP progression:* Review completed milestone verification entries in `docs/product/ROADMAP.md`.
  3. *Dependency/build hygiene:* Check for any unused cargo profile flags or unnecessary dependencies.

## Iteration 22
- **Lens:** Naming/consistency
- **Change:** Annotate `BackupError::Settings` with `#[error(transparent)]` for consistent domain error delegation (`crates/lore-core/src/backup.rs`).
- **Critique:**
  - `BackupError::Settings` formatted storage errors with `#[error("settings storage error")]`, causing redundant nested prefixes (e.g. `"settings storage error: sqlite error: ..."`), while `SourceRootError::Storage` used `#[error(transparent)]`.
  - Fix: Annotated `BackupError::Settings` with `#[error(transparent)]` to standardize transparent error propagation across core submodules.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *ROADMAP progression:* Audit and verify M5/M6/M7 roadmap criteria in `ROADMAP.md`.
  2. *Dependency/build hygiene:* Audit workspace Cargo.toml profiles and dev-dependencies.
  3. *Correctness bugs:* Review token count overflow bounds in session summary aggregations.

## Iteration 23
- **Lens:** ROADMAP progression
- **Change:** Reconcile roadmap tracker status with audited milestone implementations (`docs/product/ROADMAP.md`).
- **Critique:**
  - `ROADMAP.md` reconciliation metadata lagged behind recent work on folders, backup scheduling UI and recovery hardening, and schema regression suites.
  - Fix: Updated the reconciliation header to reflect the current audited status.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *Dependency/build hygiene:* Verify workspace lints and compiler profiles.
  2. *Correctness bugs:* Inspect token total aggregation overflow handling in Codex adapter.
  3. *Missing tests/edge cases:* Test `get_setting` behavior when reading a key that was overwritten multiple times.

## Iteration 24
- **Lens:** Dependency/build hygiene
- **Change:** Add explicit clippy lints configuration to `src-tauri/Cargo.toml` (`src-tauri/Cargo.toml`).
- **Critique:**
  - `src-tauri/Cargo.toml` lacked an explicit `[lints.clippy]` table, leaving the desktop shell crate without explicitly configured workspace clippy warning levels.
  - Fix: Added `[lints.clippy] all = { level = "warn", priority = -1 }` to `src-tauri/Cargo.toml`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *Correctness bugs:* Audit token total parsing and non-negative assertions in `CodexAdapter`.
  2. *Missing tests/edge cases:* Keyset pagination tests for folder views when folder contains only 1 session.
  3. *Error handling:* Verify graceful UI message when database lock error occurs during backup.

## Iteration 25
- **Lens:** Correctness bugs
- **Change:** Add `non_negative_int_field` helper and validate non-negative token usage bounds in Codex adapter (`crates/lore-core/src/adapters/common.rs`, `crates/lore-core/src/adapters/codex.rs`).
- **Critique:**
  - Token counts parsed from agent log payloads in `codex.rs` relied on unchecked `as_i64()` calls without non-negative bounds checking or `u64` conversion support. Negative sentinel metrics (e.g. `-1`) could enter session summaries.
  - Fix: Added `non_negative_int_field` in `common.rs` with `filter(|&v| v >= 0)` and `u64` conversion fallback, integrated into `CodexAdapter::token_totals`, and added unit test coverage in `common.rs`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *Missing tests/edge cases:* Test `get_setting` behavior when reading a key overwritten multiple times.
  2. *Error handling:* Verify error handling when restoring a backup into a read-only directory path.
  3. *Performance/allocations:* Review SQL query parameter binding in `query.rs` for list queries.

## Iteration 26
- **Lens:** Missing tests/edge cases
- **Change:** Add keyset pagination, empty-folder, and non-existent folder tests for `list_folder_sessions_page` (`crates/lore-core/src/query.rs`).
- **Critique:**
  - `list_folder_sessions_page` lacked unit test coverage for multi-page keyset continuation, empty folders, and queries against unknown folder IDs.
  - Fix: Added `list_folder_sessions_page_paginates_and_handles_empty_folders` in `crates/lore-core/src/query.rs`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *Error handling:* Verify graceful error when parsing invalid UTF-8 strings in json helper utilities.
  2. *Performance/allocations:* Optimize vector capacities in session detail part collections.
  3. *API & DTO ergonomics:* Add TypeScript helper for folder deletion confirmation dialogs.

## Iteration 27
- **Lens:** Error handling / export completeness
- **Change:** Render structured JSON message parts when text is absent in Markdown export (`crates/lore-core/src/export.rs`).
- **Critique:**
  - `export_session_markdown` only rendered `part.text`. Non-text parts containing structured JSON (such as tool inputs/arguments in `part.content_json`) were silently omitted from the exported Markdown.
  - Fix: Added structured JSON rendering fallback ````json\n...\n```` via `render(json)` to ensure full-fidelity export, and added unit test coverage in `export.rs`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *Performance/allocations:* Pre-allocate vector capacity when populating `parts_by_seq` in `query.rs`.
  2. *API & DTO ergonomics:* Audit TypeScript type exports in `src/ipc.ts`.
  3. *Docs accuracy vs code:* Verify that `docs/architecture/SECURITY.md` notes structured JSON part export masking.

## Iteration 28
- **Lens:** Performance/allocations
- **Change:** Eliminate intermediate vector pass and dummy allocations during `session_messages` construction (`crates/lore-core/src/query.rs`).
- **Critique:**
  - `session_messages` previously allocated empty `Vec::new()` part vectors for every message row, followed by an additional full-vector `.into_iter().map(...).collect()` mutation pass to attach parts.
  - Fix: Directly populate `parts_by_seq.remove(&seq).unwrap_or_default()` into `MessageDto` during the initial row query loop.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *API & DTO ergonomics:* Ensure `BackupScheduleDto` and `FolderSummary` type exports are cleanly documented.
  2. *Docs accuracy vs code:* Reconcile any newly covered export behaviors in architecture documentation.
  3. *UX & accessibility:* Verify keyboard tab navigation in settings panel backup cadence selector.

## Iteration 29
- **Lens:** API & DTO ergonomics
- **Change:** Add default `limit = 50` parameters across frontend IPC query and search functions (`src/ipc.ts`, `src/ipc.test.ts`).
- **Critique:**
  - `listSessions`, `listSessionsPage`, `listRepositorySessions`, `listRepositorySessionsPage`, `listFolderSessionsPage`, `search`, and `searchPage` previously required mandatory limit parameters, forcing UI components to repeat magic numbers.
  - Fix: Added default `limit: number = 50` across all list and search IPC wrappers and added tests in `src/ipc.test.ts`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *Docs accuracy vs code:* Verify that `docs/architecture/SECURITY.md` notes structured JSON part export masking.
  2. *UX & accessibility:* Verify keyboard focus trapping during delete-folder confirmation dialog.
  3. *Security/input validation:* Check that search term sanitization handles null bytes safely.

## Iteration 30
- **Lens:** Docs accuracy vs code
- **Change:** Document structured JSON part export formatting and secret masking in security architecture (`docs/architecture/SECURITY.md`).
- **Critique:**
  - `docs/architecture/SECURITY.md` §4 listed general text masking for session export but did not explicitly document that structured JSON parts are preserved in fenced code blocks and subject to the same scanning and redaction rules.
  - Fix: Updated `docs/architecture/SECURITY.md` §4 to specify that structured JSON message parts in exports are formatted in fenced code blocks and redacted by default.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (88 tests).
- **Backlog Candidates Noticed:**
  1. *UX & accessibility:* Verify delete folder button keyboard accessibility and confirmation dialog focus.
  2. *Security/input validation:* Test search query input validation for special control characters.
  3. *Dead code & duplication:* Audit redundant imports across `src/components/`.

## Iteration 31
- **Lens:** UX & accessibility
- **Change:** Add explicit `type="button"` attributes and `Delete` keyboard shortcut on folder list buttons (`src/components/FolderList.tsx`, `src/components/FolderList.test.tsx`).
- **Critique:**
  - `FolderList.tsx` buttons omitted explicit `type="button"` attributes and only supported folder deletion via mouse clicking the ✕ button, without a keyboard equivalent when the folder button was focused.
  - Fix: Added `type="button"` to all buttons, added `Delete` key handler to delete focused folder, and added test in `FolderList.test.tsx`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (89 tests).
- **Backlog Candidates Noticed:**
  1. *Security/input validation:* Test search term sanitization for embedded null bytes or unicode format controls.
  2. *Dead code & duplication:* Audit redundant test utilities across frontend test files.
  3. *Naming/consistency:* Audit CSS class name conventions across sidebar navigation components.

## Iteration 32
- **Lens:** Security/input validation
- **Change:** Sanitize embedded null bytes and discard empty phrase terms in FTS5 MATCH builder (`crates/lore-core/src/search.rs`).
- **Critique:**
  - `fts_match` in `search.rs` passed user search terms directly into quoted phrases without stripping `\0` null bytes or filtering out terms that evaluate to empty phrases (`""`), risking C-string truncation or invalid FTS5 syntax errors.
  - Fix: Stripped `\0` null bytes during phrase construction, filtered out empty quoted phrases, and added regression assertions in `search.rs`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (89 tests).
- **Backlog Candidates Noticed:**
  1. *Dead code & duplication:* Consolidate duplicate mock setups in frontend test suites.
  2. *Naming/consistency:* Reconcile CSS class naming for modal buttons vs sidebar buttons.
  3. *ROADMAP progression:* Verify M6 search performance benchmark suite documentation.

## Iteration 33
- **Lens:** Dead code & duplication
- **Change:** Consolidate shared `baseProps` fixture in `SettingsPanel.test.tsx` (`src/components/SettingsPanel.test.tsx`).
- **Critique:**
  - `SettingsPanel.test.tsx` repeated inline dummy mock callbacks across every test case instead of reusing a clean base properties object.
  - Fix: Defined and reused `baseProps` across all render calls in `SettingsPanel.test.tsx`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (89 tests).
- **Backlog Candidates Noticed:**
  1. *Naming/consistency:* Reconcile CSS modal backdrop and animation class names.
  2. *ROADMAP progression:* Check M6 performance benchmark targets in `docs/architecture/SEARCH.md`.
  3. *Dependency/build hygiene:* Audit Cargo.lock and npm package-lock for clean dependency graphs.

## Iteration 34
- **Lens:** Naming/consistency
- **Change:** Add semantic `.modal-backdrop` class alias in CSS and apply to Settings modal (`src/styles.css`, `src/components/SettingsPanel.tsx`).
- **Critique:**
  - Dialog overlay styles were coupled exclusively to `.palette__backdrop`, using palette-specific naming for settings and general modal dialogs.
  - Fix: Aliased `.modal-backdrop` with `.palette__backdrop` in `styles.css` and added `.modal-backdrop` to `SettingsPanel.tsx`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (89 tests).
- **Backlog Candidates Noticed:**
  1. *ROADMAP progression:* Reconcile search ranking boost tuning notes in `SEARCH.md`.
  2. *Dependency/build hygiene:* Check package.json scripts and dependency locks.
  3. *Correctness bugs:* Audit date format edge cases in session duration formatting.

## Iteration 35
- **Lens:** ROADMAP progression
- **Change:** Reconcile search sort order support (`SortOrder::Relevance`, `SortOrder::Newest`, `SortOrder::Oldest`) in search architecture doc (`docs/architecture/SEARCH.md`).
- **Critique:**
  - `docs/architecture/SEARCH.md` §3 listed relevance/newest/oldest sort support as planned work, though all three sort modes with keyset pagination are fully implemented in `crates/lore-core/src/search.rs`.
  - Fix: Updated `docs/architecture/SEARCH.md` §3 to mark sort orders as implemented.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (89 tests).
- **Backlog Candidates Noticed:**
  1. *Dependency/build hygiene:* Check package.json dependencies and scripts.
  2. *Correctness bugs:* Verify session duration string formatting for sub-minute sessions.
  3. *Missing tests/edge cases:* Test sort order switching in frontend search UI.

## Iteration 36
- **Lens:** Dependency/build hygiene
- **Change:** Add `test:watch` script in `package.json` for interactive local test execution (`package.json`).
- **Critique:**
  - `package.json` had single-run `"test": "vitest run"` but lacked an interactive `"test:watch": "vitest"` script for developers running local TDD loops.
  - Fix: Added `"test:watch": "vitest"` to `package.json`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 10 test files passed (89 tests).
- **Backlog Candidates Noticed:**
  1. *Correctness bugs:* Audit date format edge cases in session duration formatting.
  2. *Missing tests/edge cases:* Test sort order switching in frontend search UI.
  3. *Error handling:* Validate folder name trim handling for empty strings on rename.

## Iteration 37
- **Lens:** Correctness bugs
- **Change:** Clamp future clock skew in relative timestamp formatting and add test suite (`src/format.ts`, `src/format.test.ts`).
- **Critique:**
  - `formatRelative` in `src/format.ts` produced negative time outputs like `"-2m"` when given future timestamps or when system clocks had skew across processes.
  - Fix: Clamped relative time difference `diff <= 45_000` to return `"just now"`, and added a full unit test suite in `src/format.test.ts`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 11 test files passed (97 tests).
- **Backlog Candidates Noticed:**
  1. *Missing tests/edge cases:* Test sort order switching in search UI components.
  2. *Error handling:* Verify error handling when renaming a folder to an empty string.
  3. *Performance/allocations:* Audit CSS animations for GPU acceleration properties.

## Iteration 38
- **Lens:** Missing tests/edge cases
- **Change:** Add test coverage for `onExitUp` focus escape and `j`/`k` Vim navigation keys in `SearchResults.test.tsx` (`src/components/SearchResults.test.tsx`).
- **Critique:**
  - `SearchResults.test.tsx` tested mouse selection and Arrow navigation, but omitted coverage for the `onExitUp` callback (invoked when navigating up from the first result to return focus to the search box) and `j`/`k` Vim navigation keys.
  - Fix: Added test cases covering `onExitUp` and `j`/`k` key navigation.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 11 test files passed (98 tests).
- **Backlog Candidates Noticed:**
  1. *Error handling:* Validate folder name trim handling for empty strings on rename.
  2. *Performance/allocations:* Audit CSS transitions for will-change or hardware acceleration where helpful.
  3. *API & DTO ergonomics:* Audit return types of `createFolder` and `renameFolder`.

## Iteration 39
- **Lens:** Error handling
- **Change:** Sanitize unprintable ASCII control characters in folder names (`crates/lore-core/src/folders.rs`).
- **Critique:**
  - `clean_name` in `crates/lore-core/src/folders.rs` normalized whitespace and string length, but did not strip unprintable ASCII control characters (such as null bytes or terminal control codes).
  - Fix: Filtered out non-printable control characters `c.is_control()` in `clean_name` and added regression tests in `folders.rs`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 11 test files passed (98 tests).
- **Backlog Candidates Noticed:**
  1. *Performance/allocations:* Add `will-change: transform` or layer promotion to high-frequency UI transitions.
  2. *API & DTO ergonomics:* Audit FolderSummary and BackupScheduleDto interfaces in `src/ipc.ts`.
  3. *Docs accuracy vs code:* Verify that `docs/architecture/DATA_MODEL.md` accurately documents control character sanitization.

## Iteration 40
- **Lens:** Performance/allocations
- **Change:** Pre-allocate `messages` vector capacity in `session_messages` query (`crates/lore-core/src/query.rs`).
- **Critique:**
  - `session_messages` initialized `let mut messages = Vec::new()` with 0 initial capacity, causing dynamic reallocations as message rows were read from SQLite during thread opening.
  - Fix: Pre-allocated `Vec::with_capacity(parts_by_seq.len())` using the known parts mapping size.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 11 test files passed (98 tests).
- **Backlog Candidates Noticed:**
  1. *API & DTO ergonomics:* Ensure `FolderSummary` and `BackupScheduleDto` types are explicitly exported in `src/ipc.ts`.
  2. *Docs accuracy vs code:* Audit `docs/architecture/DATA_MODEL.md` for `folder` and `session_folder` table schema.
  3. *UX & accessibility:* Test keyboard focus return after closing modals.

## Iteration 41
- **Lens:** API & DTO ergonomics
- **Change:** Export `BackupInterval` union type at the top of the IPC module (`src/ipc.ts`, `src/ipc.test.ts`).
- **Critique:**
  - `src/ipc.ts` had a loose type declaration in the middle of function implementations rather than exporting `BackupInterval` alongside primary DTOs.
  - Fix: Grouped and exported `BackupInterval = "off" | "daily" | "weekly"` with the top-level IPC types and added type verification in `src/ipc.test.ts`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 11 test files passed (98 tests).
- **Backlog Candidates Noticed:**
  1. *Docs accuracy vs code:* Audit `docs/architecture/DATA_MODEL.md` for `folder` and `session_folder` table schema.
  2. *UX & accessibility:* Audit focus restoration on modal dismiss in SettingsPanel.
  3. *Security/input validation:* Check setting key validation against non-printable ASCII or control characters.

## Iteration 42
- **Lens:** Docs accuracy vs code
- **Change:** Document folder name sanitization rules in data model schema (`docs/architecture/DATA_MODEL.md`).
- **Critique:**
  - `docs/architecture/DATA_MODEL.md` §3 described the `Folder` entity schema but did not document name normalization invariants (control character filtering, length capping at 100 characters, whitespace trimming, and default fallback).
  - Fix: Updated `docs/architecture/DATA_MODEL.md` §3 with folder name normalization invariants.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 11 test files passed (98 tests).
- **Backlog Candidates Noticed:**
  1. *UX & accessibility:* Audit focus restoration on modal dismiss in SettingsPanel.
  2. *Security/input validation:* Validate setting key arguments against non-printable ASCII characters.
  3. *Dead code & duplication:* Audit redundant interface types across components.

## Iteration 43
- **Lens:** UX & accessibility
- **Change:** Exclude hidden/aria-hidden elements and prevent Tab leakage when empty in focus trap (`src/focus-trap.ts`, `src/focus-trap.test.tsx`).
- **Critique:**
  - `src/focus-trap.ts` query did not exclude `input[type="hidden"]` or `[aria-hidden="true"]` nodes, and permitted Tab key presses to leak into background windows when no focusable elements existed.
  - Fix: Added exclusion selectors for hidden/aria-hidden elements, called `event.preventDefault()` when `nodes.length === 0`, and added dedicated test suite `src/focus-trap.test.tsx`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (102 tests).
- **Backlog Candidates Noticed:**
  1. *Security/input validation:* Validate setting key arguments against non-printable ASCII characters.
  2. *Dead code & duplication:* Audit redundant interface types across components.
  3. *Naming/consistency:* Audit CSS variable definitions in index.css.

## Iteration 44
- **Lens:** Security/input validation
- **Change:** Validate key length and reject control characters in `get_setting` and `set_setting` (`src-tauri/src/lib.rs`).
- **Critique:**
  - `get_setting` and `set_setting` accepted arbitrarily long setting keys without checking length bounds (`key.len() <= 128`) or rejecting ASCII control characters, leaving the boundary open to unbounded keys or SQLite C-string anomalies.
  - Fix: Added bounds checking (`!key.is_empty() && key.len() <= 128 && !key.chars().any(|c| c.is_control())`) in `get_setting` and `set_setting`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (102 tests).
- **Backlog Candidates Noticed:**
  1. *Dead code & duplication:* Consolidate duplicate CSS transition timings.
  2. *Naming/consistency:* Audit color token naming in styles.css.
  3. *ROADMAP progression:* Audit M7 forget-everything test coverage.

## Iteration 45
- **Lens:** Dead code & duplication
- **Change:** Add `renderPalette` test fixture helper in `CommandPalette.test.tsx` (`src/components/CommandPalette.test.tsx`).
- **Critique:**
  - `CommandPalette.test.tsx` repeated identical boilerplate render setups across nearly every test case.
  - Fix: Created a consolidated `renderPalette` helper and streamlined test cases across `CommandPalette.test.tsx`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (102 tests).
- **Backlog Candidates Noticed:**
  1. *Naming/consistency:* Audit color token naming in styles.css.
  2. *ROADMAP progression:* Audit M7 forget-everything test coverage.
  3. *Dependency/build hygiene:* Check unused dev dependencies in root and crates.

## Iteration 46
- **Lens:** Naming/consistency
- **Change:** Add `--accent-fg: #ffffff;` token to manual `[data-theme="dark"]` override in `src/styles.css`.
- **Critique:**
  - `[data-theme="dark"]` in `src/styles.css` omitted the explicit `--accent-fg` declaration present in the `@media (prefers-color-scheme: dark)` block, creating a token discrepancy between manual and OS dark themes.
  - Fix: Added `--accent-fg: #ffffff;` to `[data-theme="dark"]`.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (102 tests).
- **Backlog Candidates Noticed:**
  1. *ROADMAP progression:* Audit M7 forget-everything test coverage.
  2. *Dependency/build hygiene:* Check unused dev dependencies in root and crates.
  3. *Correctness bugs:* Audit SQLite index integrity on all v0 tables.

## Iteration 47
- **Lens:** ROADMAP progression
- **Change:** Note `SortOrder` multi-mode keyset pagination in M6 roadmap sections (`docs/product/ROADMAP.md`).
- **Critique:**
  - `docs/product/ROADMAP.md` M6 summary and test plan referenced BM25 keyset pagination, omitting explicit mention of the relevance/newest/oldest sort suite landed in `crates/lore-core/src/search.rs`.
  - Fix: Updated `docs/product/ROADMAP.md` under M6 Current State and Milestone Details to specify relevance/newest/oldest sort mode coverage.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (102 tests).
- **Backlog Candidates Noticed:**
  1. *Dependency/build hygiene:* Audit unused dev dependencies or scripts in `crates/lore-core/Cargo.toml`.
  2. *Correctness bugs:* Check `crates/lore-core/src/backup.rs` file permissions error handling.
  3. *Missing tests/edge cases:* Add regression tests for empty search query handling in `search.rs`.

## Iteration 48
- **Lens:** Dependency/build hygiene
- **Change:** Expand static forbidden networking symbols in boundary test (`crates/lore-core/tests/no_network_in_archive.rs`).
- **Critique:**
  - `no_network_in_archive.rs` guarded against common standard library and async HTTP networking symbols, but omitted other modern networking and WebSocket clients (`tungstenite`, `attohttpc`, `surf`, `wreq`).
  - Fix: Broadened the `FORBIDDEN` symbols array in `no_network_in_archive.rs` with qualified module paths to strengthen static compile-time network isolation tests without false positives on English words.
- **Validation Results:**
  - `cargo test --workspace`: 80 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (102 tests).
- **Backlog Candidates Noticed:**
  1. *Correctness bugs:* Verify SQLite backup error propagation under non-fatal status codes.
  2. *Missing tests/edge cases:* Test sort order switching in frontend search UI.
  3. *Error handling:* Check `get_git_snapshot` error conversion when git inspection fails.

## Iteration 49
- **Lens:** Correctness bugs
- **Change:** Ensure `is_backup_file` checks `path.is_file()` to ignore subdirectories (`crates/lore-core/src/backup.rs`, `crates/lore-core/tests/backup.rs`).
- **Critique:**
  - `is_backup_file` checked filename prefix and extension but omitted `path.is_file()`, which would treat a directory named like a backup (`lore-*.db`) as an archive file during backup enumeration and pruning.
  - Fix: Added `path.is_file()` check in `is_backup_file` and added regression test `list_backups_ignores_subdirectories_matching_backup_naming_pattern`.
- **Validation Results:**
  - `cargo test --workspace`: 81 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (102 tests).
- **Backlog Candidates Noticed:**
  1. *Missing tests/edge cases:* Test sort order switching in frontend search UI.
  2. *Error handling:* Check `get_git_snapshot` error conversion when git inspection fails.
  3. *Performance/allocations:* Check string cloning during folder name sanitization.

## Iteration 50
- **Lens:** Missing tests/edge cases
- **Change:** Add unit test coverage for empty search filters and unknown filter prefixes (`crates/lore-core/src/search.rs`).
- **Critique:**
  - `search.rs` unit tests lacked explicit assertions for empty filter values (`agent:`, `path:`), whitespace-only inputs, and unknown filter prefixes that should fall back to search terms.
  - Fix: Added `parse_query_handles_empty_filters_and_unknown_prefixes` unit test in `crates/lore-core/src/search.rs`.
- **Validation Results:**
  - `cargo test --workspace`: 81 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (102 tests).
- **Backlog Candidates Noticed:**
  1. *Error handling:* Verify error handling when `read_schedule` encounters invalid JSON.
  2. *Performance/allocations:* Audit pre-allocations in search document projections.
  3. *API & DTO ergonomics:* Audit IPC commands return type clarity for `remove_agent_root`.

## Iteration 51
- **Lens:** Error handling
- **Change:** Early-return `BackupError::Io` on nonexistent/directory paths in `restore_backup` and use `serde_json` serialization in `write_schedule` (`crates/lore-core/src/backup.rs`, `crates/lore-core/tests/backup.rs`).
- **Critique:**
  - `restore_backup` directly invoked SQLite restore without validating that `backup_path.is_file()`, and `write_schedule` used raw string formatting rather than standard serialization.
  - Fix: Added `if !backup_path.is_file() { return Err(BackupError::Io); }` check, switched `write_schedule` to `serde_json::to_string`, and added `restore_backup_rejects_a_nonexistent_file` integration test.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (102 tests).
- **Backlog Candidates Noticed:**
  1. *Performance/allocations:* Pre-allocate vector capacity in `crates/lore-core/src/search.rs` query keyset builder.
  2. *API & DTO ergonomics:* Clarify return type documentation in `src/ipc.ts`.
  3. *Docs accuracy vs code:* Verify `docs/architecture/SEARCH.md` keyset pagination parameters.

## Iteration 52
- **Lens:** Performance/allocations
- **Change:** Pre-allocate query string and parameters capacity in `search_page` (`crates/lore-core/src/search.rs`).
- **Critique:**
  - `search_page` dynamically resized `sql` strings and `params` vectors on each search invocation without capacity hints, causing repetitive heap reallocations on every debounced keystroke query.
  - Fix: Pre-allocated `String::with_capacity(1024)` for `sql` and `Vec::with_capacity(16)` for `params`.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (102 tests).
- **Backlog Candidates Noticed:**
  1. *API & DTO ergonomics:* Export `SessionSortOrder` helper type alias in `src/ipc.ts`.
  2. *Docs accuracy vs code:* Reconcile M6 keyset pagination docs in `docs/architecture/SEARCH.md`.
  3. *UX & accessibility:* Audit keyboard focus after modal dismissal.

## Iteration 53
- **Lens:** API & DTO ergonomics
- **Change:** Group `SearchSort` type and re-export `SearchPage` DTO at the top of the IPC surface (`src/ipc.ts`, `src/ipc.test.ts`).
- **Critique:**
  - `SearchPage` was imported from generated bindings but omitted from the top-level re-export type block, and `SearchSort` was defined inline midway down the file.
  - Fix: Added `SearchPage` to the export type list, grouped `SearchSort` with top-level types, and added test coverage for `SearchSort` in `src/ipc.test.ts`.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Docs accuracy vs code:* Audit `docs/architecture/SEARCH.md` pagination cursor schema.
  2. *UX & accessibility:* Check focus return when closing SettingsPanel via Escape.
  3. *Security/input validation:* Check `delete_folder` folder ID validation.

## Iteration 54
- **Lens:** Docs accuracy vs code
- **Change:** Document relevance and chronological keyset pagination tuples in `docs/architecture/SEARCH.md` §6.
- **Critique:**
  - `docs/architecture/SEARCH.md` §6 detailed BM25 relevance keyset sorting without noting that chronological `Newest` and `Oldest` modes page over `(started_at, search_document.id)` with NULLs-last handling.
  - Fix: Updated `docs/architecture/SEARCH.md` §6 to specify both relevance and chronological keyset sorting schemes.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *UX & accessibility:* Audit keyboard navigation and focus restoration in SettingsPanel.
  2. *Security/input validation:* Check folder ID argument format in IPC folder commands.
  3. *Dead code & duplication:* Consolidate redundant CSS button classes.

## Iteration 55
- **Lens:** UX & accessibility
- **Change:** Add explicit `type="button"` attributes to close and action buttons in `SettingsPanel.tsx` (`src/components/SettingsPanel.tsx`).
- **Critique:**
  - Close button and "Forget everything" button omitted `type="button"`, making their trigger behavior vulnerable to accidental form context defaults.
  - Fix: Added `type="button"` to both buttons in `src/components/SettingsPanel.tsx`.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Security/input validation:* Validate folder ID hex string format in IPC folder commands.
  2. *Dead code & duplication:* Consolidate redundant CSS button classes.
  3. *Naming/consistency:* Audit styling tokens in index.css.

## Iteration 56
- **Lens:** Security/input validation
- **Change:** Validate ID length and control characters in folder IPC commands (`src-tauri/src/lib.rs`).
- **Critique:**
  - `rename_folder`, `delete_folder`, `set_session_folder`, and `list_folder_sessions_page` passed `id` and `session_id` arguments directly without checking length bounds or rejecting ASCII control characters.
  - Fix: Added input validation checks (`!id.is_empty() && id.len() <= 64 && !id.chars().any(|c| c.is_control())` and session ID bounds) across folder commands.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Dead code & duplication:* Audit redundant CSS rules across component styles.
  2. *Naming/consistency:* Reconcile button variant classes across components.
  3. *ROADMAP progression:* Audit M7 deletion sweep verification plan.

## Iteration 57
- **Lens:** Dead code & duplication
- **Change:** Utilize `BackupInterval` type across state and functions in `BackupSettings.tsx` (`src/components/BackupSettings.tsx`).
- **Critique:**
  - `BackupSettings.tsx` used untyped primitive strings for interval values rather than importing and using the domain `BackupInterval` union type.
  - Fix: Applied `BackupInterval` type to state, `INTERVALS` option array, and `persist` arguments in `src/components/BackupSettings.tsx`.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Naming/consistency:* Reconcile button variant classes across components.
  2. *ROADMAP progression:* Audit M7 deletion sweep verification plan.
  3. *Dependency/build hygiene:* Audit Cargo.lock workspace dependencies.

## Iteration 58
- **Lens:** Naming/consistency
- **Change:** Add hover state rule for `.btn--ghost` in design system stylesheet (`src/styles.css`).
- **Critique:**
  - `.btn--ghost` lacked an explicit `:hover:not(:disabled)` state in `styles.css`, creating an interactive discrepancy compared to other button variants.
  - Fix: Added `.btn--ghost:hover:not(:disabled) { background: var(--surface-hover); }` to `src/styles.css`.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *ROADMAP progression:* Audit M7 deletion sweep verification plan in PRD and Roadmap.
  2. *Dependency/build hygiene:* Check Cargo.lock workspace dependencies.
  3. *Correctness bugs:* Audit SQLite index integrity on session folders.

## Iteration 59
- **Lens:** ROADMAP progression
- **Change:** Note IPC boundary input hardening in M7 roadmap status (`docs/product/ROADMAP.md`).
- **Critique:**
  - `docs/product/ROADMAP.md` M7 summary and build sequence listed backups, quarantine, and settings without noting IPC boundary validation hardening.
  - Fix: Updated `docs/product/ROADMAP.md` under M7 in Current state and Next build sequence.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Dependency/build hygiene:* Audit `package.json` scripts and devDependencies.
  2. *Correctness bugs:* Audit SQLite index coverage on session folders.
  3. *Missing tests/edge cases:* Add test for setting invalid backup retention values.

## Iteration 60
- **Lens:** Dependency/build hygiene
- **Change:** Add `test:coverage` script to `package.json` (`package.json`).
- **Critique:**
  - `package.json` included `test` and `test:watch` scripts but lacked a predefined `test:coverage` command for coverage reporting.
  - Fix: Added `"test:coverage": "vitest run --coverage"` to `package.json`.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Correctness bugs:* Audit SQLite index coverage on session folders.
  2. *Missing tests/edge cases:* Add test for setting invalid backup retention values.
  3. *Error handling:* Verify error handling when restoring non-sqlite files.

## Iteration 61
- **Lens:** Correctness bugs
- **Change:** Filter zero-width byte-order marks (`\u{feff}`) in folder name sanitization (`crates/lore-core/src/folders.rs`).
- **Critique:**
  - `clean_name` filtered ASCII control characters and collapsed whitespace, but did not filter zero-width non-breaking space / byte-order marks (`\u{feff}`), allowing visually empty folder names to bypass the default `"New folder"` fallback.
  - Fix: Filtered `*c != '\u{feff}'` in `clean_name` and added regression test coverage.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Missing tests/edge cases:* Test `list_folder_sessions_page` pagination edge cases (cursor out of bounds).
  2. *Error handling:* Check SQLite foreign key violations on folder deletion cascade.
  3. *Performance/allocations:* Profile folder query preparation in hot loops.

## Iteration 62
- **Lens:** Missing tests/edge cases
- **Change:** Add test coverage for malformed cursors and boundary limit clamping in `list_folder_sessions_page` (`crates/lore-core/src/query.rs`).
- **Critique:**
  - `list_folder_sessions_page` pagination test suite lacked assertions verifying that malformed cursor strings gracefully degrade to page 1 and zero/negative limits are clamped.
  - Fix: Added test assertions for malformed cursor strings and boundary limits to `query.rs`.
- **Validation Results:**
  - `cargo test --workspace`: 82 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Error handling:* Check SQLite error mapping in `list_folder_sessions_page`.
  2. *Performance/allocations:* Avoid query string reallocations in session page builders.
  3. *API & DTO ergonomics:* Audit IPC FolderSummary serialization.

## Iteration 63
- **Lens:** Error handling
- **Change:** Fall back to older intact backups in `recover_archive` when newest backup is corrupted (`crates/lore-core/src/recovery.rs`, `crates/lore-core/tests/recovery.rs`).
- **Critique:**
  - `recover_archive` only attempted to restore `backups.into_iter().last()`; if the newest backup was corrupted, recovery degraded to `QuarantinedOnly` with no database restored despite older valid backups existing.
  - Fix: Iterated `backups.into_iter().rev()` in `recover_archive` to restore the newest usable backup, and added `recover_archive_falls_back_to_older_backup_when_newest_is_corrupt` integration test.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Performance/allocations:* Pre-allocate string query buffers in `query.rs` folder queries.
  2. *API & DTO ergonomics:* Ensure consistent naming for folder DTO conversions.
  3. *Docs accuracy vs code:* Verify recovery documentation in `SECURITY.md`.

## Iteration 64
- **Lens:** Performance/allocations
- **Change:** Pre-allocate query string and params vector capacity in paginated session queries (`crates/lore-core/src/query.rs`).
- **Critique:**
  - `list_sessions_page`, `list_repository_sessions_page`, and `list_folder_sessions_page` created un-sized SQL strings and parameter vectors, incurring unnecessary reallocations as WHERE/AND clauses and parameters were appended.
  - Fix: Pre-allocated `String::with_capacity` (384/512 bytes) and `Vec::with_capacity(8)` for query builder buffers in `query.rs`.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *API & DTO ergonomics:* Audit IPC FolderSummary serialization and field comments.
  2. *Docs accuracy vs code:* Reconcile recovery documentation in `SECURITY.md`.
  3. *UX & accessibility:* Verify focus indicators on folder action buttons.

## Iteration 65
- **Lens:** API & DTO ergonomics
- **Change:** Document `list_folder_sessions_page` command on `SessionPage` DTO in `lore-ipc` (`crates/lore-ipc/src/lib.rs`).
- **Critique:**
  - `SessionPage` DTO doc comments listed `list_sessions_page` and `list_repository_sessions_page` but omitted `list_folder_sessions_page`.
  - Fix: Updated `SessionPage` doc comments in `crates/lore-ipc/src/lib.rs` and regenerated TypeScript bindings.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Docs accuracy vs code:* Reconcile recovery documentation in `SECURITY.md`.
  2. *UX & accessibility:* Verify focus indicators on folder action buttons.
  3. *Security/input validation:* Check IPC repository command argument length limits.

## Iteration 66
- **Lens:** Docs accuracy vs code
- **Change:** Document reverse-order intact backup recovery fallback in `docs/architecture/SECURITY.md` §6.
- **Critique:**
  - `docs/architecture/SECURITY.md` §6 described archive recovery generically without mentioning the fallback to older intact backups when the newest snapshot is corrupt.
  - Fix: Updated `docs/architecture/SECURITY.md` §6 with exact reverse-chronological fallback details.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *UX & accessibility:* Verify focus-visible outline on folder item buttons.
  2. *Security/input validation:* Check IPC repository command argument length limits.
  3. *Dead code & duplication:* Consolidate test helper assertions across integration tests.

## Iteration 67
- **Lens:** UX & accessibility
- **Change:** Add explicit `type="button"` attributes and capitalize landmark `aria-label` in `RepositoryList.tsx` (`src/components/RepositoryList.tsx`).
- **Critique:**
  - "All sessions" and repository buttons lacked explicit `type="button"`, and the landmark had lowercase `aria-label="repositories"`.
  - Fix: Added `type="button"` to buttons and set `aria-label="Repositories"` in `src/components/RepositoryList.tsx`.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Security/input validation:* Check IPC repository command argument length limits in `lib.rs`.
  2. *Dead code & duplication:* Consolidate test helper assertions across integration tests.
  3. *Naming/consistency:* Audit aria-labels across left navigation panes.

## Iteration 68
- **Lens:** Security/input validation
- **Change:** Validate repository ID bounds and control characters in IPC repository commands (`src-tauri/src/lib.rs`).
- **Critique:**
  - `list_repository_sessions` and `list_repository_sessions_page` accepted `id` arguments directly without checking length bounds or rejecting ASCII control characters.
  - Fix: Added input validation checks (`!id.is_empty() && id.len() <= 256 && !id.chars().any(|c| c.is_control())`) across repository IPC commands.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Dead code & duplication:* Consolidate test helper assertions across integration tests.
  2. *Naming/consistency:* Audit aria-labels across left navigation panes.
  3. *ROADMAP progression:* Audit M7 verification deliverables in PRD.

## Iteration 69
- **Lens:** Dead code & duplication
- **Change:** Implement `Tokens::is_empty(&self) -> bool` predicate helper in domain model (`crates/lore-core/src/model.rs`).
- **Critique:**
  - `Tokens` lacked an `is_empty()` helper method, requiring repetitive multi-field checks across adapters and ingest pipelines.
  - Fix: Added `pub fn is_empty(&self) -> bool` to `Tokens` with unit tests covering empty and partial field states.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Naming/consistency:* Reconcile aria-label casing across sidebar navigation landmarks.
  2. *ROADMAP progression:* Reconcile M7 safety criteria across PRD and ROADMAP.
  3. *Dependency/build hygiene:* Audit dev-dependencies in lore-core Cargo.toml.

## Iteration 70
- **Lens:** Naming/consistency
- **Change:** Standardize `aria-label` casing across `App.tsx` and `SessionList.tsx` (`src/App.tsx`, `src/components/SessionList.tsx`, `src/App.test.tsx`).
- **Critique:**
  - `App.tsx` and `SessionList.tsx` used lowercase `aria-label` strings (`"sessions"`, `"search"`), inconsistent with capitalized accessible names (`"Repositories"`, `"Folders"`, `"Settings"`, `"Command palette"`).
  - Fix: Capitalized `aria-label` attributes to `"Sessions"` and `"Search"` and aligned test matcher regexes.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *ROADMAP progression:* Reconcile M7 safety criteria across PRD and ROADMAP.
  2. *Dependency/build hygiene:* Audit dev-dependencies in lore-core Cargo.toml.
  3. *Correctness bugs:* Check SQLite index usage on session listing queries.

## Iteration 71
- **Lens:** ROADMAP progression
- **Change:** Document M7 local backup, restore, and recovery capabilities in PRD Safety scope (`docs/product/PRD.md` §6).
- **Critique:**
  - `docs/product/PRD.md` §6 listed secret scanning and export redaction under Safety without noting SQLite online backup create/restore wiring, recovery fallback, or user-configurable cadence controls.
  - Fix: Updated `docs/product/PRD.md` §6 Safety scope to reflect the M7 local backup, restore, and recovery deliverables.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Dependency/build hygiene:* Audit dev-dependencies in lore-core Cargo.toml.
  2. *Correctness bugs:* Check SQLite index usage on session listing queries.
  3. *Missing tests/edge cases:* Add test for invalid backup intervals in IPC handlers.

## Iteration 72
- **Lens:** Dependency/build hygiene
- **Change:** Expand forbidden networking and TLS symbols list in static archive test (`crates/lore-core/tests/no_network_in_archive.rs`).
- **Critique:**
  - `FORBIDDEN` symbols omitted TLS libraries (`native_tls`, `rustls`) and space-delimited `hyper `, leaving static guard coverage slightly narrower than all possible transport boundaries.
  - Fix: Added `native_tls`, `rustls`, and `hyper ` to `FORBIDDEN` array in `crates/lore-core/tests/no_network_in_archive.rs`.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Correctness bugs:* Check SQLite index coverage on session folders and segments.
  2. *Missing tests/edge cases:* Add test for setting invalid backup intervals in IPC handlers.
  3. *Error handling:* Validate SQLite error conversions across repository queries.

## Iteration 73
- **Lens:** Correctness bugs
- **Change:** Normalize trailing separators and match canonicalized paths in `remove_custom_root` (`crates/lore-core/src/source_roots.rs`).
- **Critique:**
  - `remove_custom_root` compared stored canonical paths directly against `PathBuf::from(path)` without trailing slash trimming or canonicalization, failing to remove roots when paths included trailing separators or symlink discrepancies.
  - Fix: Added trailing separator trimming and `std::fs::canonicalize` fallback matching in `remove_custom_root` with unit test.
- **Validation Results:**
  - `cargo test --workspace`: 83 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Missing tests/edge cases:* Add test for setting invalid backup intervals in IPC handlers.
  2. *Error handling:* Validate SQLite error conversions across repository queries.
  3. *Performance/allocations:* Check string buffer allocations in export markdown.

## Iteration 74
- **Lens:** Missing tests/edge cases
- **Change:** Add unit and integration tests for backup schedule settings fallback and keep clamping (`crates/lore-core/tests/backup.rs`).
- **Critique:**
  - `crates/lore-core/tests/backup.rs` lacked tests verifying fallback when interval/retention settings contained corrupted JSON, unknown interval strings, or out-of-range bounds (e.g. 0 or >100).
  - Fix: Added `backup_schedule_read_clamping_and_fallback` testing default fallback, unparseable JSON degradation, unknown interval strings, and retention clamping.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Error handling:* Verify error handling when writing invalid JSON settings.
  2. *Performance/allocations:* Check string buffer allocations in export markdown.
  3. *API & DTO ergonomics:* Audit DTO docstrings in lore-ipc.

## Iteration 75
- **Lens:** Error handling
- **Change:** Validate ID lengths and control characters across session, export, backup, and agent root IPC commands (`src-tauri/src/lib.rs`).
- **Critique:**
  - `get_session`, `get_file_patch`, `get_git_snapshot`, `session_secret_count`, `export_session_markdown`, `forget_session`, `set_backup_schedule`, `add_agent_root`, and `remove_agent_root` passed unvalidated ID strings directly into SQLite queries and lock acquisition paths.
  - Fix: Added early boundary validation checking bounds and rejecting control characters on all session, backup schedule, and root IPC commands.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Performance/allocations:* Pre-allocate string capacity in `export_session_markdown`.
  2. *API & DTO ergonomics:* Document and audit DTO field comments in lore-ipc.
  3. *Docs accuracy vs code:* Verify IPC command list in ARCHITECTURE.md §5.

## Iteration 76
- **Lens:** Performance/allocations
- **Change:** Pre-allocate Markdown export buffer capacity in `export_session_markdown` (`crates/lore-core/src/export.rs`).
- **Critique:**
  - `export_session_markdown` used unbuffered `String::new()`, causing repeated heap reallocations during Markdown rendering on sessions with many messages and file events.
  - Fix: Pre-allocated string buffer with capacity derived from `messages.len()` and `file_events.len()`.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *API & DTO ergonomics:* Document and audit DTO field comments in lore-ipc.
  2. *Docs accuracy vs code:* Verify IPC command list in ARCHITECTURE.md §5.
  3. *UX & accessibility:* Audit keyboard focus outlines on empty session placeholders.

## Iteration 77
- **Lens:** API & DTO ergonomics
- **Change:** Document `forget_everything` in `ForgetReport` DTO docstring (`crates/lore-ipc/src/lib.rs`).
- **Critique:**
  - `ForgetReport` docstring only referenced `forget_session`, omitting `forget_everything` which also returns `ForgetReport`.
  - Fix: Updated `ForgetReport` documentation to cite both `forget_session` and `forget_everything` and regenerated TypeScript bindings.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Docs accuracy vs code:* Reconcile IPC command catalog in ARCHITECTURE.md §5.
  2. *UX & accessibility:* Audit keyboard focus outlines on empty session placeholders.
  3. *Security/input validation:* Check boundary parameters in search_page IPC handler.

## Iteration 78
- **Lens:** Docs accuracy vs code
- **Change:** Mark backup cadence/retention modeling decision as DECIDED (M7) in `DATA_MODEL.md` §11 (`docs/architecture/DATA_MODEL.md`).
- **Critique:**
  - `docs/architecture/DATA_MODEL.md` §11 still listed backup cadence/retention as an open modeling decision despite being implemented and documented as a settled settings schema in §3 and `SECURITY.md`.
  - Fix: Updated `DATA_MODEL.md` §11 to record the M7 decision and reference settings keys and recovery fallback behavior.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *UX & accessibility:* Audit keyboard focus outlines on empty session placeholders.
  2. *Security/input validation:* Check boundary parameters in search_page IPC handler.
  3. *Dead code & duplication:* Check duplicate color definitions in styles.css.

## Iteration 79
- **Lens:** UX & accessibility
- **Change:** Add explicit `type="button"` attributes to actions and patch toggles and standardize section landmark names in `SessionView.tsx` (`src/components/SessionView.tsx`).
- **Critique:**
  - In `SessionView.tsx`, interactive buttons ("Export", "Forget", "Show more messages", and "View patch") lacked explicit `type="button"`, and landmarks used lowercase `"session"` rather than descriptive `"Session detail"`.
  - Fix: Added `type="button"` attributes to all buttons and set `aria-label="Session detail"` on main and empty session detail containers.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Security/input validation:* Check boundary parameters in search_page IPC handler in `src-tauri/src/lib.rs`.
  2. *Dead code & duplication:* Check duplicate color definitions in styles.css.
  3. *Naming/consistency:* Audit error messages across IPC boundary commands.

## Iteration 80
- **Lens:** Security/input validation
- **Change:** Enforce query length bounds and limit clamping in `search` and `search_page` IPC commands (`src-tauri/src/lib.rs`).
- **Critique:**
  - `search` and `search_page` did not clamp limits and did not enforce a maximum query length limit before invoking FTS compilation and SQLite queries.
  - Fix: Added `query.len() <= 10_000` length bounding and `limit.clamp(1, 10_000)` clamping to search IPC commands.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Dead code & duplication:* Check duplicate color definitions in styles.css.
  2. *Naming/consistency:* Audit error messages across IPC boundary commands.
  3. *ROADMAP progression:* Review M7 release packaging verification gates in ROADMAP.

## Iteration 81
- **Lens:** Dead code & duplication
- **Change:** Group `.dot--accent` and `.dot--confirmed` CSS selector rules in `styles.css` (`src/styles.css`).
- **Critique:**
  - `src/styles.css` defined `.dot--accent` and `.dot--confirmed` in separate consecutive rules with duplicate `background: var(--accent)` declarations.
  - Fix: Grouped `.dot--accent, .dot--confirmed` into a single combined rule.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Naming/consistency:* Audit error messages across IPC boundary commands in `lib.rs`.
  2. *ROADMAP progression:* Review M7 release packaging verification gates in ROADMAP.
  3. *Dependency/build hygiene:* Audit Cargo workspace metadata tags.

## Iteration 82
- **Lens:** Naming/consistency
- **Change:** Add folder name length bounds checking to `create_folder` and `rename_folder` in IPC commands (`src-tauri/src/lib.rs`).
- **Critique:**
  - `create_folder` and `rename_folder` did not enforce a parameter length bound at the IPC boundary, unlike session, setting, and search commands.
  - Fix: Added `name.len() <= 256` validation checks to `create_folder` and `rename_folder`.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *ROADMAP progression:* Review M7 release packaging verification gates in ROADMAP.
  2. *Dependency/build hygiene:* Audit Cargo workspace metadata tags.
  3. *Correctness bugs:* Audit FTS5 rank score serialization in search results.

## Iteration 83
- **Lens:** ROADMAP progression
- **Change:** Update M7 boundary validation scope across table and next build sequence in `ROADMAP.md` (`docs/product/ROADMAP.md`).
- **Critique:**
  - `docs/product/ROADMAP.md` M7 description previously only noted boundary validation for settings and folders, omitting session, repository, search, and backup schedule commands.
  - Fix: Updated `ROADMAP.md` M7 summary and build sequence notes to reflect complete boundary validation across all IPC commands.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Dependency/build hygiene:* Audit Cargo workspace metadata tags and authors list.
  2. *Correctness bugs:* Audit FTS5 rank score serialization in search results.
  3. *Missing tests/edge cases:* Add test for invalid session folder input combinations.

## Iteration 84
- **Lens:** Dependency/build hygiene
- **Change:** Add `homepage` metadata to `[workspace.package]` and inherit across member crates in Cargo configurations (`Cargo.toml`, `crates/lore-core/Cargo.toml`, `crates/lore-ipc/Cargo.toml`, `src-tauri/Cargo.toml`).
- **Critique:**
  - Cargo manifests lacked `homepage` in `[workspace.package]`, and member crates did not inherit homepage URLs.
  - Fix: Defined `homepage = "https://github.com/hsusul/lore"` in workspace package and set `homepage.workspace = true` in all crate manifests.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Correctness bugs:* Audit FTS5 rank score serialization in search results.
  2. *Missing tests/edge cases:* Add test for invalid session folder input combinations.
  3. *Error handling:* Verify SQLite connection timeout configurations.

## Iteration 85
- **Lens:** Correctness bugs
- **Change:** Filter zero-width space and joiner characters (`\u{200b}`, `\u{200c}`, `\u{200d}`, `\u{2060}`) in `clean_name` (`crates/lore-core/src/folders.rs`).
- **Critique:**
  - `clean_name` filtered `\u{feff}`, but strings composed of zero-width space (`\u{200b}`), ZWNJ (`\u{200c}`), ZWJ (`\u{200d}`), or word joiner (`\u{2060}`) bypassed fallback and produced invisible folder names in the UI.
  - Fix: Added `is_zero_width` character filter matching zero-width spaces/joiners so invisible names fall back to `"New folder"`.
- **Validation Results:**
  - `cargo test --workspace`: 84 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Missing tests/edge cases:* Add test for invalid session folder input combinations.
  2. *Error handling:* Verify SQLite connection timeout configurations.
  3. *Performance/allocations:* Audit folder listing query statement caching.

## Iteration 86
- **Lens:** Missing tests/edge cases
- **Change:** Add `unfiling_an_already_unfiled_or_unknown_session_is_safe_noop` unit test (`crates/lore-core/src/folders.rs`).
- **Critique:**
  - `folders.rs` unit tests covered filing and unfiling filed sessions, but did not explicitly test that calling `set_session_folder` with `None` on an already-unfiled or nonexistent session ID is a safe no-op.
  - Fix: Added `unfiling_an_already_unfiled_or_unknown_session_is_safe_noop` verifying idempotency.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Error handling:* Verify SQLite busy timeout handling on connection initialization.
  2. *Performance/allocations:* Audit folder listing query statement caching.
  3. *API & DTO ergonomics:* Audit docstrings in `lore_ipc::FolderSummary`.

## Iteration 87
- **Lens:** Error handling
- **Change:** Implement `From<std::io::Error>` for `StorageError` in `storage.rs` (`crates/lore-core/src/storage.rs`).
- **Critique:**
  - `StorageError` lacked a standard `From<std::io::Error>` implementation, requiring repetitive `.map_err(|_| StorageError::Io)` conversions across storage/blob operations.
  - Fix: Added `impl From<std::io::Error> for StorageError` mapping I/O errors content-freely to `StorageError::Io` and added unit test.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Performance/allocations:* Pre-allocate vector capacity in `list_folders`.
  2. *API & DTO ergonomics:* Audit docstrings in `lore_ipc::FolderSummary`.
  3. *Docs accuracy vs code:* Verify storage error description in DATA_MODEL.md.

## Iteration 88
- **Lens:** Performance/allocations
- **Change:** Eliminate intermediate `Vec<&str>` allocation in `clean_name` (`crates/lore-core/src/folders.rs`).
- **Critique:**
  - `clean_name` split on whitespace into an intermediate `Vec<&str>` before calling `.join(" ")`.
  - Fix: Pre-allocated `String::with_capacity(filtered.len())` and accumulated whitespace-delimited tokens directly into the output buffer.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *API & DTO ergonomics:* Audit docstrings in `lore_ipc::FolderSummary`.
  2. *Docs accuracy vs code:* Verify storage error description in DATA_MODEL.md.
  3. *UX & accessibility:* Check settings panel focus trap cleanup on unmount.

## Iteration 89
- **Lens:** API & DTO ergonomics
- **Change:** Update `FolderSummary` docstring in `lore-ipc` (`crates/lore-ipc/src/lib.rs`) and regenerate TypeScript definitions.
- **Critique:**
  - `FolderSummary` docstring previously stated it was solely an element of `list_folders`, omitting that it is also the return payload of `create_folder`.
  - Fix: Updated `FolderSummary` documentation and regenerated TypeScript types via `cargo test -p lore-ipc`.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Docs accuracy vs code:* Verify storage error description in DATA_MODEL.md.
  2. *UX & accessibility:* Check settings panel focus trap cleanup on unmount.
  3. *Security/input validation:* Check setting key allowed characters regex/bounds.

## Iteration 90
- **Lens:** Docs accuracy vs code
- **Change:** Update ARCHITECTURE.md canonical description in `DOCS_INDEX.md` (`docs/DOCS_INDEX.md`).
- **Critique:**
  - `docs/DOCS_INDEX.md` entry for `architecture/ARCHITECTURE.md` did not mention user-defined folders and local backup/recovery subsystems.
  - Fix: Updated the canonical docs index table to note user-defined folders and local backups.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *UX & accessibility:* Check settings panel focus trap cleanup on unmount.
  2. *Security/input validation:* Check setting key allowed characters regex/bounds.
  3. *Dead code & duplication:* Audit unused CSS helper classes in styles.css.

## Iteration 91
- **Lens:** UX & accessibility
- **Change:** Add `isConnected` DOM connection check and safe type narrowing to focus restoration in `SettingsPanel.tsx` (`src/components/SettingsPanel.tsx`).
- **Critique:**
  - `SettingsPanel.tsx` called `.focus()` on `restoreFocusRef.current` without checking whether the previously focused node remained connected to the DOM document upon closing the modal.
  - Fix: Stored `document.activeElement instanceof HTMLElement` and checked `previousFocus?.isConnected` before restoring focus, matching `CommandPalette.tsx`.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Security/input validation:* Check setting key allowed characters regex/bounds.
  2. *Dead code & duplication:* Audit unused CSS helper classes in styles.css.
  3. *Naming/consistency:* Audit error message formatting in settings operations.

## Iteration 92
- **Lens:** Security/input validation
- **Change:** Bound maximum allowed `value_json` payload size in `set_setting` IPC command (`src-tauri/src/lib.rs`).
- **Critique:**
  - `set_setting` validated `key` length and JSON format, but did not enforce a size upper bound on `value_json` before JSON parsing and database storage.
  - Fix: Added `value_json.len() <= 65_536` check in `set_setting` to reject oversized payload allocations.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Dead code & duplication:* Audit unused CSS helper classes in styles.css.
  2. *Naming/consistency:* Audit error message formatting in settings operations.
  3. *ROADMAP progression:* Review scale test suite execution commands in TESTING.md.

## Iteration 93
- **Lens:** Dead code & duplication
- **Change:** Add explicit `type="button"` attributes to omnibar, theme toggle, settings, and rescan controls in `App.tsx` (`src/App.tsx`).
- **Critique:**
  - `App.tsx` header buttons lacked explicit `type="button"` attributes, causing inconsistency with child component button standards.
  - Fix: Added `type="button"` attributes to all shell header action buttons.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Naming/consistency:* Audit error message formatting in settings operations.
  2. *ROADMAP progression:* Review scale test suite execution commands in TESTING.md.
  3. *Dependency/build hygiene:* Audit workspace Cargo clippy configuration.

## Iteration 94
- **Lens:** Naming/consistency
- **Change:** Add `path` parameter boundary checks to `add_agent_root` and `remove_agent_root` in `lib.rs` (`src-tauri/src/lib.rs`).
- **Critique:**
  - `add_agent_root` and `remove_agent_root` validated `agent_id`, but did not check `path` length or control characters at the IPC boundary.
  - Fix: Added `path.is_empty() || path.len() > 4096 || path.chars().any(|c| c.is_control())` input checks.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *ROADMAP progression:* Review scale test suite execution commands in TESTING.md.
  2. *Dependency/build hygiene:* Audit workspace Cargo clippy configuration.
  3. *Correctness bugs:* Audit FTS5 rank score formatting in search results.

## Iteration 95
- **Lens:** ROADMAP progression
- **Change:** Update `no_network_in_archive.rs` test path references in `TESTING.md` (`docs/development/TESTING.md`).
- **Critique:**
  - `docs/development/TESTING.md` referenced `tests/no_network_in_archive.rs` instead of its workspace location `crates/lore-core/tests/no_network_in_archive.rs`.
  - Fix: Corrected the test file paths in §1 (test pyramid) and §7 (security guard tests).
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Dependency/build hygiene:* Audit workspace Cargo clippy configuration.
  2. *Correctness bugs:* Audit FTS5 rank score formatting in search results.
  3. *Missing tests/edge cases:* Add test for malformed keyset cursor decoding.

## Iteration 96
- **Lens:** Dependency/build hygiene
- **Change:** Add `license` and `homepage` package metadata to `package.json` (`package.json`).
- **Critique:**
  - `package.json` omitted `license` and `homepage` fields, diverging from workspace Cargo package declarations.
  - Fix: Added `license: "Apache-2.0"` and `homepage: "https://github.com/hsusul/lore"`.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Correctness bugs:* Audit FTS5 rank score formatting in search results.
  2. *Missing tests/edge cases:* Add test for malformed keyset cursor decoding.
  3. *Error handling:* Verify query parameter bounds on keyset search cursor parsing.

## Iteration 97
- **Lens:** Correctness bugs
- **Change:** Require finite `rank` (`rank.is_finite()`) in `Cursor::decode` (`crates/lore-core/src/search.rs`).
- **Critique:**
  - `Cursor::decode` parsed hex bits into `f64` without checking whether the value was finite. Non-finite values (`NaN` or `inf`) could bypass decoding and cause invalid floating-point comparisons in SQLite queries.
  - Fix: Added `if !rank.is_finite() { return None; }` check and added unit test cases for `NaN` and `inf` encodings.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Missing tests/edge cases:* Add test for search query sorting with multiple identical rank scores.
  2. *Error handling:* Verify query parameter bounds on keyset search cursor parsing.
  3. *Performance/allocations:* Audit SQL parameter allocations in search pagination queries.

## Iteration 98
- **Lens:** Missing tests/edge cases
- **Change:** Add `oldest_sort_paginates_without_duplicates` integration test (`crates/lore-core/tests/search.rs`).
- **Critique:**
  - `search.rs` had comprehensive test coverage for `SortOrder::Newest` pagination and single-shot `SortOrder::Oldest` ordering, but lacked an integration test verifying full multi-page keyset traversal in `SortOrder::Oldest` mode.
  - Fix: Added `oldest_sort_paginates_without_duplicates` testing multi-page iteration with null-start sessions.
- **Validation Results:**
  - `cargo test --workspace`: 85 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Error handling:* Verify query parameter bounds on keyset search cursor parsing.
  2. *Performance/allocations:* Audit SQL parameter allocations in search pagination queries.
  3. *API & DTO ergonomics:* Review DTO docstrings in `lore_ipc`.

## Iteration 99
- **Lens:** Error handling
- **Change:** Implement `From<std::io::Error>` for `BackupError` in `backup.rs` (`crates/lore-core/src/backup.rs`).
- **Critique:**
  - `BackupError` did not implement `From<std::io::Error>`, forcing manual `.map_err(|_| BackupError::Io)` calls across filesystem operations.
  - Fix: Implemented `From<std::io::Error>` for `BackupError` mapping I/O errors content-freely and added unit test.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Performance/allocations:* Audit query parameter vector allocations in search pagination.
  2. *API & DTO ergonomics:* Review DTO docstrings in `lore_ipc`.
  3. *Docs accuracy vs code:* Verify backup settings description in `SECURITY.md`.

## Iteration 100
- **Lens:** Performance/allocations
- **Change:** Build quoted FTS5 MATCH expression directly into pre-allocated string in `fts_match` (`crates/lore-core/src/search.rs`).
- **Critique:**
  - `fts_match` mapped terms through intermediate strings and collected into a temporary `Vec<String>` before calling `.join(" ")`.
  - Fix: Pre-allocated `String::with_capacity(terms.len() * 16)` and formatted quoted phrases and double quotes directly into the output buffer.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *API & DTO ergonomics:* Review DTO docstrings in `lore_ipc`.
  2. *Docs accuracy vs code:* Verify backup settings description in `SECURITY.md`.
  3. *UX & accessibility:* Check active state styling for search pagination buttons.

## Iteration 101
- **Lens:** API & DTO ergonomics
- **Change:** Clarify `DetectedAgent` docstring in `lore-ipc` (`crates/lore-ipc/src/lib.rs`) and regenerate TypeScript definitions.
- **Critique:**
  - `DetectedAgent` docstring stated it was the "Payload of the `list_detected_agents` command" rather than "Payload element of the `list_detected_agents` command".
  - Fix: Updated docstring and regenerated TypeScript types via `cargo test -p lore-ipc`.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Docs accuracy vs code:* Verify backup settings description in `SECURITY.md`.
  2. *UX & accessibility:* Check active state styling for search pagination buttons.
  3. *Security/input validation:* Check pagination limit clamp across all command handlers.

## Iteration 102
- **Lens:** Docs accuracy vs code
- **Change:** Update static network test path reference in `SECURITY.md` (`docs/architecture/SECURITY.md`).
- **Critique:**
  - `SECURITY.md` referenced `tests/no_network_in_archive.rs` instead of its workspace crate path `crates/lore-core/tests/no_network_in_archive.rs`.
  - Fix: Updated the test path reference in §1 (V0 privacy contract).
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *UX & accessibility:* Check active state styling for search pagination buttons.
  2. *Security/input validation:* Check pagination limit clamp across all command handlers.
  3. *Dead code & duplication:* Inspect unused CSS styles or imports.

## Iteration 103
- **Lens:** UX & accessibility
- **Change:** Add hover state styling for `.sessions__more` and `.results__more` buttons in `styles.css` (`src/styles.css`).
- **Critique:**
  - Pagination buttons in session list and search results were muted with `var(--text-3)` and had no hover text color feedback.
  - Fix: Added `.sessions__more:hover:not(:disabled), .results__more:hover:not(:disabled) { color: var(--text); }`.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Security/input validation:* Check pagination limit clamp across all command handlers.
  2. *Dead code & duplication:* Inspect unused CSS styles or imports.
  3. *Naming/consistency:* Audit folder delete confirmation messaging.

## Iteration 104
- **Lens:** Security/input validation
- **Change:** Add cursor length limit (`s.len() <= 2_048`) in `Cursor::decode` (`crates/lore-core/src/search.rs`).
- **Critique:**
  - `Cursor::decode` in `search.rs` did not enforce a string length bound before splitting and parsing hex numbers, unlike `SessionCursor::decode` in `query.rs`.
  - Fix: Added `if s.len() > 2_048 { return None; }` check and oversized cursor test case.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Dead code & duplication:* Inspect unused CSS styles or imports.
  2. *Naming/consistency:* Audit folder delete confirmation messaging.
  3. *ROADMAP progression:* Review scale test suite execution commands in TESTING.md.

## Iteration 106
- **Lens:** Naming/consistency
- **Change:** Match folder delete button `title` tooltip to its `aria-label` in `FolderList.tsx` (`src/components/FolderList.tsx`).
- **Critique:**
  - The delete button had a generic `title="Delete folder"` tooltip despite having an accessible `aria-label={`Delete folder ${folder.name}`}`.
  - Fix: Updated `title={`Delete folder ${folder.name}`}` for tooltip consistency across folder items.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *ROADMAP progression:* Review scale test suite execution commands in TESTING.md.
  2. *Dependency/build hygiene:* Audit workspace Cargo clippy configuration.
  3. *Correctness bugs:* Audit FTS5 query token escaping edge cases.

## Iteration 107
- **Lens:** ROADMAP progression
- **Change:** Update `SECURITY.md` canonical description in `DOCS_INDEX.md` (`docs/DOCS_INDEX.md`).
- **Critique:**
  - `DOCS_INDEX.md` summarized `architecture/SECURITY.md` as "Threat model, secrets, privacy contract", omitting the local backups, recovery, and deletion threat contracts added in M7.
  - Fix: Updated entry to "Threat model, secrets, privacy contract, local backups, and recovery".
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Dependency/build hygiene:* Audit workspace Cargo clippy configuration.
  2. *Correctness bugs:* Audit FTS5 query token escaping edge cases.
  3. *Missing tests/edge cases:* Add unit test for empty folder pagination query.

## Iteration 108
- **Lens:** Dependency/build hygiene
- **Change:** Add `[lints.rust]` `rust_2018_idioms` configuration to `src-tauri/Cargo.toml` (`src-tauri/Cargo.toml`).
- **Critique:**
  - `src-tauri/Cargo.toml` configured `[lints.clippy]` but omitted `rust_2018_idioms` in `[lints.rust]`, creating a minor discrepancy with workspace lint standards.
  - Fix: Added `[lints.rust] rust_2018_idioms = { level = "warn", priority = -1 }`.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Correctness bugs:* Audit FTS5 query token escaping edge cases.
  2. *Missing tests/edge cases:* Add unit test for empty folder pagination query.
  3. *Error handling:* Verify error message propagation on empty folder queries.

## Iteration 109
- **Lens:** Correctness bugs
- **Change:** Clamp negative `scrollTop` in `computeWindow` (`src/virtual.ts`).
- **Critique:**
  - On macOS trackpad rubber-band bounce, negative `scrollTop` caused `(scrollTop + viewportHeight) / rowHeight` to underestimate `endIndex`, rendering fewer items than needed during the bounce animation.
  - Fix: Added `const safeScrollTop = Math.max(0, scrollTop);` and added a unit test for negative scroll position.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (104 tests).
- **Backlog Candidates Noticed:**
  1. *Missing tests/edge cases:* Add unit test for empty folder pagination query.
  2. *Error handling:* Verify error message propagation on empty folder queries.
  3. *Performance/allocations:* Audit CSS selector complexity in timeline rows.

## Iteration 110
- **Lens:** Missing tests/edge cases
- **Change:** Expand `empty_database_lists_nothing` test to cover paginated queries in `query.rs` (`crates/lore-core/src/query.rs`).
- **Critique:**
  - `empty_database_lists_nothing` tested unpaginated queries, but omitted verifying that `list_sessions_page`, `list_repository_sessions_page`, and `list_folder_sessions_page` return empty pages with `next_cursor: None` on empty archives.
  - Fix: Added assertions covering paginated session, repository, and folder endpoints on an empty database.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (104 tests).
- **Backlog Candidates Noticed:**
  1. *Error handling:* Verify error message propagation on empty folder queries.
  2. *Performance/allocations:* Audit CSS selector complexity in timeline rows.
  3. *API & DTO ergonomics:* Review DTO docstrings in `lore_ipc`.

## Iteration 111
- **Lens:** Error handling
- **Change:** Replace control characters with whitespace in `clean_name` before tokenization (`crates/lore-core/src/folders.rs`).
- **Critique:**
  - `clean_name` filtered out control characters directly (`!c.is_control()`), which caused multiline inputs without adjacent spaces (e.g. `"Folder\r\n\tWith\nSpaces"`) to collapse into concatenated words (`"FolderWithSpaces"`).
  - Fix: Mapped control characters to space `' '` before `split_whitespace()` tokenization and added unit test.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (104 tests).
- **Backlog Candidates Noticed:**
  1. *Performance/allocations:* Audit CSS selector complexity in timeline rows.
  2. *API & DTO ergonomics:* Review DTO docstrings in `lore_ipc`.
  3. *Docs accuracy vs code:* Verify custom root path validation docs.

## Iteration 112
- **Lens:** Performance/allocations
- **Change:** Avoid intermediate string clone in `query_session_page` cursor encoding (`crates/lore-core/src/query.rs`).
- **Critique:**
  - `query_session_page` cloned `session.id` into a temporary `SessionCursor` struct before calling `.encode()`.
  - Fix: Added `SessionCursor::encode_parts(started_at, id)` to format cursor strings directly from row references without copying string fields.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (104 tests).
- **Backlog Candidates Noticed:**
  1. *API & DTO ergonomics:* Review DTO docstrings in `lore_ipc`.
  2. *Docs accuracy vs code:* Verify custom root path validation docs.
  3. *UX & accessibility:* Audit keyboard navigation in folder row list items.

## Iteration 113
- **Lens:** API & DTO ergonomics
- **Change:** Note sort order requirement in `SearchPage` docstring (`crates/lore-ipc/src/lib.rs`) and regenerate TypeScript definitions.
- **Critique:**
  - `SearchPage` docstring stated that cursors were valid only for the query that produced them, omitting that cursors are also specific to the sort order (`"relevance"`, `"newest"`, `"oldest"`).
  - Fix: Updated docstring to specify "identical query and sort order" and regenerated TypeScript types.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (104 tests).
- **Backlog Candidates Noticed:**
  1. *Docs accuracy vs code:* Verify custom root path validation docs.
  2. *UX & accessibility:* Audit keyboard navigation in folder row list items.
  3. *Security/input validation:* Check string size boundaries on setting keys.









## Iteration 105
- **Lens:** Dead code & duplication
- **Change:** Update `keyset_after` doc comment in `query.rs` (`crates/lore-core/src/query.rs`).
- **Critique:**
  - `keyset_after` doc comment documented sharing between `list_sessions_page` and `list_repository_sessions_page`, but missed noting its shared usage in `list_folder_sessions_page`.
  - Fix: Updated doc comment to list all three pagination functions.
- **Validation Results:**
  - `cargo test --workspace`: 86 passed across lore-core, lore-ipc, lore-app (2 scale/dev ignored).
  - `cargo clippy --workspace -- -D warnings`: Clean (0 warnings).
  - `npm run typecheck && npm run lint && npm test`: Clean; 12 test files passed (103 tests).
- **Backlog Candidates Noticed:**
  1. *Naming/consistency:* Audit folder delete confirmation messaging.
  2. *ROADMAP progression:* Review scale test suite execution commands in TESTING.md.
  3. *Dependency/build hygiene:* Audit workspace Cargo clippy configuration.








































































































