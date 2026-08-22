# Testing Strategy

> The testing contract for the implemented archive and its remaining V0 acceptance work. The spine is **anonymized fixtures** so we never need a developer's real coding history to test parsing. Companion: `docs/agents/*` (per-adapter fixture lists), `ARCHITECTURE.md`. Tags: **DECISION** / **OPINION**.

## 1. Test pyramid

| Layer | Scope | Tooling | Runs |
|---|---|---|---|
| **Unit** | pure functions: parsers, tokenizer, git identity, secret rules, normalizers | Rust `cargo test`; TS `vitest` | every commit |
| **Fixture/golden** | adapter parse → normalized rows, programmatic asserts over anonymized fixtures | Rust integration (`cargo test`); snapshot tooling (`insta`) planned | every commit |
| **Migration** | schema up/down + data preserved across versions | Rust integration on temp DBs | every commit |
| **Property/fuzz** | parsers vs malformed/truncated/huge input | fuzz/property harnesses (`proptest` / `cargo-fuzz`) **planned**; tolerant-parsing tests ship as fixtures today | nightly + PR-on-parser-change |
| **Integration** | discovery→ingest→git→index over a fixture "home dir" | Rust integration; injected `DiscoveryRoots` | every commit |
| **Performance** | scan/search at 1k/10k/100k synthetic sessions | deterministic synthetic generator + `cargo test`; criterion baselines **planned** | nightly + release |
| **UI/component** | components, keyboard nav, states | vitest + Testing Library / Playwright component | PR |
| **E2E** | launch→scan→browse→search on a synthetic profile | Tauri driver / Playwright | pre-release |
| **Security guards** | capability/dependency boundary, OS-level egress, permissions, secrets, deletion | static call-site guard (`crates/lore-core/tests/no_network_in_archive.rs`); OS-level deny-egress harness **planned** (ROADMAP M7) | every commit / release |

## 2. Fixture strategy (the core)

**DECISION — everything parser-related is tested on committed, anonymized fixtures**, never on `~/.claude`/`~/.codex` directly.

### 2.1 Categories (per adapter — see `docs/agents/<AGENT>.md` §"fixtures")
Minimal happy-path · mixed ordered content blocks · string-vs-array user content · tool-call/result (success + error, out of order) · thinking/reasoning metadata · sidechain/subagent (Claude) · patch/file-events (Codex map shape) · optional/partial session git · encrypted-region exclusion · compaction · cwd change/segments · truncated-final-line · unknown-future-type (→ `partial`) · multi-MB perf · planted-secret · title variants · non-git cwd.

### 2.2 Anonymization pipeline (DECISION)
A `fixtures` tool that takes a real session and produces a shareable fixture:
1. **Structure-preserving redaction:** replace prompt/code/tool text with synthetic tokens of similar shape/length; keep JSON structure, types, event ordering, field presence intact.
2. **Path/identity scrubbing:** rewrite cwd/paths/remotes to `/repo/...`, `example.com/org/repo`; stable-hash real ids → fake stable ids.
3. **Secret planting/removal:** strip any real secrets; where a secret test is needed, insert **known-fake** patterns (documented test keys, never live).
4. **Provenance:** each fixture records the `agent_version` it represents + a note; golden output stored alongside.
- Result: fixtures are safe to commit publicly and exercise real structural edge cases.

### 2.3 Golden/snapshot tests
Parse fixture → serialize normalized rows → assert against expected values inline. `insta` snapshot adoption is planned; today golden assertions are programmatic (assert_eq on parsed fields), which forces an explicit review on every expected-value change.

## 3. Adapter-version compatibility (DECISION)
- Fixtures are tagged by `agent_version`. When a new agent version introduces a field/type, add a fixture at that version; parsing branches on version only when required.
- A **forward-compat test** feeds an "unknown future" event and asserts graceful `partial` + a recorded note — never a crash. This is the contract from `AGENT_ADAPTERS.md` §5.

## 4. Git integration tests (DECISION)
Spin up **real temp git repos** in tests (init, commit, branch, worktree add, rebase, delete) and assert:
- linked worktrees sharing `git_common_dir` group into one Repository;
- remote+root evidence can relink an unambiguous moved/ordinary clone;
- forks sharing a root commit remain separate without stronger evidence or user confirmation;
- N worktrees (shared `.git`) → one Repository;
- deleted branch / rebased (GC'd) commit → correct flags, history still usable;
- detached HEAD, non-git cwd, submodule (innermost-repo rule).
Use `gix` + the hardened system-Git fallback. A hostile repo config fixture defines fsmonitor, external diff/textconv, credential helpers, aliases, and protocol URLs; tests assert no helper execution, prompt, write, or network attempt.

## 5. Malformed / adversarial input
- Truncated final line; append after checkpoint; truncate/rewrite after checkpoint; move to archive path; corrupted JSON mid-file; giant single line; deeply nested content; unknown enum values; duplicate ids; out-of-order parent refs; path-traversal in `FileEvent.path` (`../../etc`) → sanitized. Assert idempotency and that checkpoints never advance past committed rows. Fuzz the parsers (currently via malformed-input fixture corpus; `proptest`/`cargo-fuzz` adoption planned).

## 6. Performance tests (targets from ROADMAP)
- A **synthetic dataset generator** (seedable, deterministic) builds 1k/10k/100k-session homes with realistic size distributions (incl. multi-MB sessions).
- Assert: 10k-session scan completes/streams without OOM and stays incremental; **search <200 ms at ~1M messages**; list scroll stays smooth (virtualized).
- Track regressions with criterion baselines in CI (planned; today the deterministic generator + `cargo test` asserts scale/latency targets directly).

## 7. Security guard tests (DECISION — these gate the privacy promise)
- **Network boundary:** resolved dependency/capability check fails if a non-updater module gains a networking/raw-socket path; webview CSP denies remote content. Today this is enforced by the static call-site guard `crates/lore-core/tests/no_network_in_archive.rs` (scans `lore-core/src` for networking symbols) plus dependency choice. An OS-level deny-egress integration test that runs scan/browse/search/export and fails on any attempted connection is **planned** (ROADMAP M7 "offline/egress acceptance").
- **Secret leakage:** planted secrets at ordinary, streaming-chunk-boundary, and middle-of-large-blob positions must be flagged and absent from SearchDocument/FTS, default export, rendered caches, and logs. Forced scanner failure quarantines the content. Canonical archive retention is expected and tested separately.
- **Permissions/threat boundary:** app-data dir `0700`, files `0600`; UI text must not claim this stops same-user processes. Warn on known sync roots without claiming path detection covers all backups.
- **Deletion/recovery:** “Forget everything” removes DB/WAL/SHM, blobs, local backups, caches, and logs; reports original logs/external exports that remain. Corruption recovery preserves quarantine and works from local backup without source logs.
- **Read-only source:** assert adapters never open agent files writable.

## 8. What we do NOT test against
- Real user history in CI (privacy + non-determinism). Local dev may point at real dirs behind a flag for manual QA, but never in automated suites.

## 9. Coverage posture (OPINION)
Chase **meaningful** coverage on parsers, normalization, git identity, secret rules, and migrations (the correctness-critical core) — not a global percentage. Every bug fixed gets a regression fixture. Green CI must mean: parsers are faithful+tolerant, git identity is correct, migrations are safe, and the privacy guards hold.
