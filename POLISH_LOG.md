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






