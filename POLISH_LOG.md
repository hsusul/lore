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
























