# Security & Privacy Threat Model

> Lore concentrates source code, prompts, tool output, reasoning, and leaked credentials. This document is the enforceable V0 boundary, including what Lore **does not** protect. Companions: `LOCAL_FIRST.md`, `SECRET_SCANNING.md`, ADR-0005. Tags: **DECISION** / **OPINION**.

## 1. V0 privacy contract

> Archive content stays on the machine. In the default configuration Lore opens no outbound connection. The only V0 network capability is a separately bounded update client, invoked manually or enabled explicitly by the user, and it sends no archive data.

This is narrower and more testable than “no networking crate appears anywhere,” because the updater legitimately needs one. Enforcement:

1. **Capability separation:** ingest, adapters, Git, storage, search, secrets, jobs, and UI-domain crates do not depend on the network client or raw socket APIs. The updater is a separate crate/module behind an IPC command and a build-time dependency allowlist.
2. **Default off:** automatic update checks are disabled on first run. Manual “Check for updates” is an explicit network action; enabling scheduled checks shows the endpoint and fields sent.
3. **Webview boundary:** CSP defaults to `default-src 'self'`, `connect-src ipc: http://ipc.localhost` (exact Tauri schemes finalized in scaffold), with remote navigation/content and arbitrary `fetch` blocked. No remote images/fonts/scripts.
4. **Dependency/source guard:** CI checks the resolved dependency graph, capability ownership, web assets, raw socket/process APIs, and updater call sites. A grep alone is insufficient.
5. **Runtime egress test (planned — ROADMAP M7 "offline/egress acceptance"):** release CI runs scan/browse/search/export and hostile-input fixtures under an OS-level deny-by-default egress harness; the test fails on any connection attempt. A separate test permits only the mocked update endpoint after explicit invocation and asserts the request fields. Today the guard is the static call-site scan `crates/lore-core/tests/no_network_in_archive.rs` plus dependency choice; the OS-level harness is still owed (ROADMAP M7).
6. **No telemetry/account/remote crash reporter/LLM in V0.** Future off-machine data flows require a new ADR and explicit opt-in.

## 2. Assets and threat boundary

Protected assets: canonical archive content, secret findings, Git/path/remotes, exports, blobs, local backups, settings, and update integrity.

V0 mitigates accidental exfiltration, cross-account filesystem access, unsafe rendering/parsing, index/export amplification, and update compromise. It does **not** protect against:

- malware or another process running as the same OS user;
- an unlocked account or administrator/root access;
- offline disk theft without platform disk encryption;
- secrets that the detector does not recognize;
- copies in original agent logs, user-chosen exports, Time Machine, or other backups outside Lore's ownership.

Permissions (`0700` directory, `0600` files) block other ordinary OS accounts, not same-user processes. Recommend FileVault and a locked login; do not market permissions as at-rest encryption. V0 does not add application-level database encryption.

## 3. Threats and controls

| Threat | Control |
|---|---|
| accidental network exfiltration | capability-separated updater, CSP, dependency/call-site guard, OS-level egress test, default-off update checks |
| secret amplification through search/export | complete local scan before SearchDocument/export, deterministic masking, quarantine on scan failure; no claim of perfect detection |
| malicious JSONL/SQLite/blob | memory-safe parsers, bounded depth/line/blob/output/time, no eval/active HTML, path normalization, fuzz/property fixtures |
| parser failure affecting other agents | adapter errors are contained as data-level failures; panic catch is a last-resort process guard, **not a sandbox**; time/memory cancellation boundaries remain required |
| prompt injection in displayed text | render inert text; Lore never executes archive instructions; future MCP labels returned content untrusted |
| same-user/local theft | out of V0 boundary; app-private permissions + FileVault guidance; do not overclaim |
| archive corruption | WAL/integrity checks, preserve quarantine, local rolling backups, recovery/salvage path; source re-scan is best-effort only |
| hostile Git repository config | `gix` primary; narrow hardened Git subprocess allowlist with protocols/helpers/hooks disabled and tests |
| update-channel attack | pinned updater public key, signed/notarized artifacts, TLS, explicit endpoint |
| opaque agent content | store only if fidelity requires; mark `opaque_excluded`; never decode/index/render/export |

## 4. Canonical archive and secret posture

Lore's canonical archive is a faithful local copy and can contain cleartext secrets. The scanner prevents **amplification**:

- address blobs by a cryptographic digest (BLAKE3): a blob address is a dedupe key carrying a `scan_state`, so a forgeable address is a redaction bypass — colliding content inherits a clean scan instead of being scanned;
- scan every persisted cleartext field/blob, including non-indexed thinking;
- build only redacted SearchDocument projections;
- default exports mask findings (including structured JSON message parts formatted in fenced code blocks) and require a fresh explicit override to include flagged content;
- "Save file" writes that same masked Markdown to a path chosen in the OS save dialog; the Rust core validates that the destination is absolute and strictly outside the Lore archive directory and agent discovery roots; the written file is a user-chosen export, outside Lore's ownership, and is not swept by "Forget everything".
- application logs accept no raw archive fields;
- reveal canonical content only after a user action in the local UI.

Scanner false negatives remain possible. UI/copy must say “flagged secrets redacted,” not “secret-free.” Rules, streaming coverage, quarantine, and tests are canonical in `SECRET_SCANNING.md`.

## 5. Filesystem and source handling

- Resolve Claude/Codex configurable roots (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) plus defaults. Request only necessary macOS access and show denied adapters.
- A custom source root requires an explicit native directory-picker action. The webview receives only the selected path, not a general filesystem API; the Tauri capability grants `dialog:allow-open` and `dialog:allow-save` (user-initiated open/save dialogs only) but no filesystem, shell, updater, or network permission. The Rust core accepts only a registered adapter and an existing absolute directory below the filesystem root, persists the canonical path locally, and opens matching source files read-only. Removing a custom root stops future scanning but does not delete archived sessions or original logs.
- Open agent sources read-only; never rename/delete/lock them. Treat them as mutable and perishable: agents append, move, archive, truncate, and delete transcripts.
- Cursor live SQLite (V1) uses `mode=ro` with WAL-aware reads and busy timeout, or the SQLite backup API to a Lore temp snapshot. `immutable=1` is allowed only for a proven closed/static copy, never the live database.
- App-owned data lives under `~/Library/Application Support/Lore/` with restrictive permissions. Detect known sync roots, but acknowledge Time Machine/backup behavior cannot be inferred solely from path.
- Original agent logs are **not** an immutable or guaranteed recovery source. Claude Code and Gemini CLI both default to retention cleanup; once ingested, Lore's archive may be the only copy.

## 6. Data durability, deletion, and recovery (DECISION)

- **Recovery:** integrity failure closes the active DB, preserves it as a quarantine artifact, and restores from the newest intact Lore-owned local backup (falling back in reverse chronological order if the newest snapshot is damaged), or preserves the quarantine artifact for best-effort salvage/re-scan if no usable backup exists. Never discard the only archive automatically.
- **Local backups:** use SQLite's online backup mechanism; backups inherit app permissions and secret posture. Cadence/retention is user-visible and bounded; no cloud upload.
- **Forget session/repo:** transactionally remove canonical rows, SearchDocuments/FTS, unreferenced blobs, findings, and derived caches; run WAL checkpoint plus `secure_delete`/vacuum maintenance; delete Lore-owned backups that could still contain the forgotten rows, then create a fresh post-deletion backup if enabled.
- **Forget everything:** close connections, remove Lore-owned DB/WAL/SHM, blobs, backups, caches, and content-bearing logs. Recreate only empty settings/state after confirmation. Secure physical erasure cannot be guaranteed on SSD/copy-on-write filesystems; say so.
- Original agent logs and external user-chosen exports are outside Lore's ownership. The confirmation names those remaining copies and offers to reveal their locations; Lore never deletes them.

## 7. Update data flow (the sole V0 network path)

When explicitly invoked/enabled:

```text
Updater → documented HTTPS release-manifest endpoint
request: Lore version, platform, architecture, release channel
response: signed release metadata
```

No stable device id, repo/path, agent inventory, session count, query, or archive content. The endpoint and actual request fixture are release-audited. Disabling checks keeps all archive functionality offline indefinitely.

## 8. Future watchlist

Each item needs a separate ADR/data-flow review before implementation: cloud/local LLM skill synthesis (privacy model remains OPEN), telemetry, crash reporting, local MCP, cross-device/team sync, remote content previews, and plugin execution.

## 9. M0/M7 security acceptance criteria

- A planted network dependency/call in any non-updater module fails CI; an obfuscated/raw-socket attempt fails the runtime egress test.
- Default first launch and full archive workflow attempt zero outbound connections.
- Explicit mocked update check sends only documented fields.
- Hostile HTML/path/JSON/Git-config fixtures cannot execute code, escape paths, invoke helpers, or access the network.
- Planted secrets in middle/chunk-boundary large blobs are absent from search/default export/cache/logs; forced scan failure quarantines content.
- Forget tests cover DB, WAL/SHM, blobs, backups, caches, and logs and report non-owned remaining copies.
- Recovery tests prove the app never assumes missing source logs can rebuild the archive.
