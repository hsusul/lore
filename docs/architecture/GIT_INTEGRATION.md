# Git Integration

> Git is Lore's differentiator, not decoration. This doc defines repository/worktree identity, evidence provenance, capture timing, and lifecycle behavior. Companions: `DATA_MODEL.md`, `SECURITY.md`, and ADR-0006. Tags: **DECISION** / **INFERENCE** / **OPINION**.

## 1. Two hard truths

1. A path is not a repository identity: repositories move, clones live at multiple paths, and linked worktrees share a Git common directory.
2. A retrospective scan cannot reconstruct an exact historical dirty tree when the agent log did not record it. Lore must never label state observed hours or months later as “the diff at session time.”

The product therefore models **identity evidence** and **Git evidence provenance**, not a fictional omniscient snapshot.

## 2. Repository and worktree identity (DECISION)

`Repository.identity_key` is an opaque Lore-generated identifier that remains stable inside the archive. Automatic matching uses multiple evidence rows; no single Git value is globally unique.

| Signal | Use | Confidence / limitation |
|---|---|---|
| resolved `git_common_dir` | Group linked worktrees of the same local repository instance | high locally; path changes when a checkout moves |
| normalized remote URL + root commit set | Match ordinary clones of the same upstream history | high; forks/mirrors and rewritten roots need care |
| normalized remote URL alone | Match shallow clones when roots are unavailable | medium; remotes can change or be shared |
| root commit set alone | Candidate match for local-only repositories | low; forks and unrelated grafted histories can share a root |
| canonical path fingerprint | Last-resort continuity hint | low; paths move and can be reused |

Rules:

1. All worktrees resolving to one `git_common_dir` attach to one Repository immediately.
2. A remote+root match may merge a newly discovered checkout automatically when it has no conflicting evidence.
3. A root-only match **never** silently merges repositories. Lore creates a candidate link and either keeps them separate or asks the user to confirm.
4. Conflicting remotes, multiple root commits, shallow history, grafts, and replace refs lower confidence and remain visible in diagnostics.
5. User-confirmed merge/split decisions are stored as identity evidence and outrank heuristics.

This deliberately gives up the false claim that “root commit = canonical repository.” It preserves the useful behavior—moves, clones, and worktrees can reconcile—without conflating forks.

## 3. Worktree resolution (DECISION)

Given a recorded `cwd` for a session segment:

1. Resolve symlinks without escaping the allowed local filesystem boundary.
2. Discover the innermost Git worktree containing that directory.
3. Resolve `.git` directory or `gitdir:` pointer, then `commondir`, to obtain `git_common_dir`.
4. Upsert a Worktree by `(repository_id, canonical_path)` and retain previous paths as identity evidence after moves.
5. If discovery is ambiguous or the path is gone, keep the segment and its recorded cwd with `repository_id=NULL` or a low-confidence candidate; never guess silently.

Sessions can change directory mid-conversation. Resolution therefore attaches to `SessionSegment`, not only to `AgentSession`; see `DATA_MODEL.md`.

## 4. Git evidence and time semantics (DECISION)

| Evidence kind | Source | What it may claim | Observation time |
|---|---|---|---|
| `agent_recorded` | fields written by the agent (for example Codex `session_meta.git`, Claude `gitBranch`) | only the fields present in that event | the event timestamp, with agent-reported provenance |
| `lore_captured` | Lore queries the repository | branch/HEAD/dirty/file summary **at capture**, not at session time | `observed_at` |
| `lore_reverified` | Lore later checks recorded commits/refs | whether an earlier recorded commit/ref still exists and what moved | `observed_at` |

> `agent_patch` is **not** a GitObservation source. Agent-recorded patch/change events (Codex `patch_apply_end.changes`) surface as `file_event` rows with `source = agent_patch` (or `agent_tool_input`), byte-faithful via the blob store — see `DATA_MODEL.md`. GitObservation carries only the three sources above.

Every Git observation stores `source`, `observed_at`, optional `event_ts`, and `temporal_confidence`. The UI must label them as **recorded by agent**, **observed by Lore at ingest**, or **reverified later**.

For V0:

- Preserve full patches only when the source log already records them.
- Lore's default repository capture stores branch, HEAD, dirty flag, ahead/behind when safe, and a changed-file/status summary. It does **not** store a full working-tree diff.
- An optional, size-bounded full-diff capture is deferred until its privacy/storage behavior is approved; it must never be implied by the default V0 promise.
- Live watching can reduce the delay between event and capture, but a close timestamp raises confidence rather than changing provenance.

## 5. Enrichment engine and safe Git boundary (DECISION)

- **Primary: `gix` (gitoxide).** Use library APIs for discovery, refs, commit walking, object checks, dirtiness, and worktree metadata. Pure-Rust execution ensures no repo-local `.gitattributes` clean/textconv filters execute.
- **System `git` fallback is narrow and hardened.** Allowlist exact read-only subcommands (`rev-parse`, `rev-list`, `for-each-ref`, `cat-file`). `status` and `diff` are dropped so executable filter scripts are never invoked. Use argument arrays, a sanitized environment, timeouts/output caps, and command-line config overrides that deny all transports and neutralize executable config (`core.fsmonitor=`, `core.hooksPath=/dev/null`, `core.pager=cat`, `diff.external=`, `credential.helper=`, `protocol.allow=never`, `gc.auto=0`). Dirtiness is taken exclusively via pure-Rust `gix` and omitted (`None`) on system fallback.
- Override executable configuration that can run helpers (`core.fsmonitor`, `core.hooksPath`, `diff.external`, textconv and credential helpers); ignore system/global config where the command does not require it. Test against hostile repository config. Absolute paths are required for capture and re-verification.
- Lore never fetches, checks out, commits, runs hooks, invokes aliases, mutates refs/index/worktree, or writes optional locks.
- If safe execution cannot be guaranteed for a repository, skip the fallback, retain agent-recorded evidence, and mark enrichment partial.

## 6. Lifecycle behavior

| Event | Behavior |
|---|---|
| branch deleted or moved | keep recorded branch; append a reverified observation and visible status |
| recorded commit rebased/GC'd | keep SHA; set verification result to missing without rewriting history |
| worktree disappears | mark Worktree missing; retain sessions and identity evidence |
| repository moves | relink only on non-conflicting evidence; retain old path evidence |
| root history is rewritten | add the new root evidence; do not silently change identity or merge another repo |
| fork shares a root | remain separate unless remote+root evidence or user confirmation establishes equivalence |
| repository is deleted | archive remains browsable from Lore-owned data; Git-dependent views degrade |
| detached HEAD | store commit with branch NULL |
| non-Git cwd | keep the segment under “No repository” |

## 7. Search and UI contract

Search can filter by repository, worktree, recorded/captured branch, recorded commit, and `FileEvent.path`. Results that depend on Git evidence expose the evidence source and observation time. A useful query is:

> Every session with recorded edits under `auth/` on recorded branch `billing`, plus any repository state Lore observed near those events.

The UI must not collapse `agent_recorded`, `agent_patch`, and `lore_captured` into a single unlabeled “session-time” rail.

## 8. Performance

- Cache discovery and identity evidence per local repository instance, not globally by root commit.
- Batch low-priority re-verification and never block transcript reads/search.
- Parse and content-address agent-recorded patches once; keep large patch bodies in scanned blobs.
- At the 10k-session target, segment-level capture is coalesced by a per-session `cwd` cache (one capture per distinct segment path); the richer `(worktree, time window, HEAD, status fingerprint)` coalescing strategy is planned.

## 9. Acceptance criteria for M4

- A fork sharing a root commit is not silently merged with its upstream.
- Multiple linked worktrees group correctly; a session that changes cwd creates multiple segments.
- A retrospective Claude session never claims an exact historical commit/dirty diff absent recorded evidence.
- A Codex agent-recorded patch remains byte-faithful and separately labeled from Lore capture.
- Hostile Git config cannot cause network access or arbitrary helper execution in fallback tests.
- Rebase/GC/move/delete scenarios append evidence and flags without overwriting historical rows.

## 10. Open questions (OPEN)

- Submodule/monorepo attribution: proposed default is the innermost repository plus an optional superproject relation.
- Full working-tree diff capture: privacy, size, and retention policy must be approved before enabling; default remains off.
- Remote normalization for self-hosted aliases and credential-bearing URLs needs a fixture matrix.
