# Agent Adapters

> The extension point that keeps Lore from becoming "a pile of Claude-specific parsing." All agent-specific code lives behind this interface. Companion: `docs/agents/*` (per-agent facts), `DATA_MODEL.md`, ADR-0003. Tags: **DECISION** / **INFERENCE** / **OPINION**.

## 1. Principle

An **adapter is read-only**. It knows one agent's on-disk format and nothing about the UI, DB, or search. It turns files into normalized entities and reports what it *can* and *cannot* provide via `capabilities()`. The rest of Lore is capability-driven: the UI and search degrade gracefully when a field is absent (e.g. no token usage for an agent that doesn't record it).

Each adapter owns its documented default roots. Lore's source-root policy adds persisted, user-selected directories to those defaults and passes the resulting effective list into the adapter; a custom root never disables a default. Folder selection and persistence stay outside the adapter, and neither path grants it write access.

## 2. The interface

```rust
/// Stable adapter identity, matching the `agent.id` primary key.
pub struct AgentId(pub &'static str); // "claude-code", "codex", ...

/// What this adapter can extract. Drives UI/search degradation.
pub struct Capabilities {
    pub messages: bool, pub thinking: bool, pub tool_calls: bool,
    pub file_events: bool, pub token_usage: bool, pub cost: bool,
    pub model_name: bool, pub summaries: bool, pub git_context: bool,
    pub message_tree: bool, pub durations: bool, pub encrypted_regions: bool,
}

/// Static adapter metadata (documentation + discovery hints).
pub struct AgentMetadata { pub display_name: &'static str, pub format_id: &'static str, pub doc_link: &'static str }

pub trait AgentAdapter: Send + Sync {
    fn id(&self) -> AgentId;
    fn metadata(&self) -> AgentMetadata;

    /// Probe for an installation under the given roots. Cheap, side-effect-free.
    fn detect_installation(&self, roots: &DiscoveryRoots) -> Detection;

    fn capabilities(&self) -> Capabilities;

    /// The effective roots this adapter scans (its documented defaults when the
    /// overrides are empty). Used for watches and owner resolution.
    fn roots(&self, overrides: &DiscoveryRoots) -> Vec<PathBuf>;

    /// Enumerate candidate session files WITHOUT parsing them. Idempotent and
    /// cheap; used for first scan + rescans.
    fn discover_sessions(&self, roots: &DiscoveryRoots) -> Vec<SessionRef>;

    /// Parse already-read content into the normalized model. Tolerant: unknown
    /// event/block/version → Partial, never panic.
    fn parse_content(&self, content: &str, fallback_dedupe: &str) -> ParsedSession;

    /// Parse one session file. Default: reads once and delegates to
    /// [`AgentAdapter::parse_content`]; adapters need not override it.
    fn parse_session(&self, source: &SessionRef) -> Result<ParsedSession, AdapterError>;
}
```

Supporting types:

```rust
pub struct Detection { pub installed: bool, pub version: Option<String>, pub roots_found: Vec<PathBuf> }

pub struct SessionRef { pub agent: AgentId, pub path: PathBuf, pub mtime: Option<SystemTime>, pub size: u64, pub native_id: Option<String> }

/// Content-free adapter error (never embeds file content or paths).
pub enum AdapterError { Io, Unreadable }

/// Registry construction error when duplicate adapter IDs are registered.
pub enum RegistryError { DuplicateId(&'static str) }

/// The normalized, DB-agnostic parse result (see DATA_MODEL.md).
pub struct ParsedSession { /* agent_session, segments, messages, parts, tool_calls, file_events, git observations, parse_status */ }
```

> Implementation note: parsing is **input-content-oriented**, not sink-based. `parse_content(&str, …)` takes already-read bytes (the ingest layer has them for hashing/checkpointing) and returns a `ParsedSession`; `parse_status` (`ok | partial | failed`) travels on that result. The earlier sink/streaming design (`IngestSink`, `ParseOutcome`, `incremental_hint`) was replaced for simplicity — adapters are pure functions over content.

## 3. Capability matrix (from the format research)

| Capability | Claude Code | Codex | Gemini CLI | Cursor | OpenCode |
|---|---|---|---|---|---|
| messages | ✅ | ✅ | ✅ | ✅ (blob) | ✅ |
| thinking | ✅ (`thinking` block) | ✅ (`reasoning`) | ? | ? | ? |
| tool_calls | ✅ | ✅ | ✅ | ? | ✅ (parts) |
| file_events | ⚠️ derive from tool_use | ✅ `patch_apply_end.changes` map (patch payload) | ⚠️ only when checkpointing enabled | ❌/derive | ⚠️ parts + snapshot git |
| token_usage | ✅ rich (`usage` incl. cache) | ✅ `token_count` | ? | ? | ✅ (cost derived) |
| model_name | ✅ | ✅ | ✅ | ✅ | ✅ |
| git_context | ⚠️ `gitBranch` only | ⚠️ optional/partial `session_meta.git` | ⚠️ optional shadow-git, not user repo identity | ❌ | ⚠️ git-root projectHash; branch/commit TBD |
| message_tree | ✅ `parentUuid` | ❌ linear | ❌ | ? | ✅ parent/subagent |
| durations | ❌ | ❌ (turn latency only) | ? | ❌ | ? |
| encrypted_regions | ❌ | ✅ (must exclude) | ❌ | ❌ | ❌ |
| Stability | Med-High | Med | Low-Med (30d TTL) | **Low** | Low-Med (no TTL, evolving) |
| Difficulty | **Low** | Low-Med | Med | **High** | Med |

✅ present · ⚠️ derivable/partial · ❌ absent · ? unverified · INF = inference. Source: `research/AGENT_STORAGE_FORMATS.md`.

## 4. Adapter lifecycle in the pipeline

```
Discovery: for adapter in registry.enabled(): adapter.discover_sessions(roots) → SessionRef*
Watcher:   notify(path) → find owning adapter by root/glob → schedule ingest(SessionRef)
Ingest:    adapter.parse_session(ref) → ParsedSession
             → buffer bounded normalized rows → git-enrich → complete secret-scan/projection
             → canonical rows + findings + SearchDocument/FTS + checkpoint (one txn)
             → set parse_status from ParsedSession; publish events after commit
```

## 5. Versioning & robustness rules (DECISION)

1. **Never hard-fail on the unknown.** Unknown top-level `type`, unknown content block, unknown `payload.type`, or a newer `agent_version` → record a bounded, content-free `note`, skip that unit, keep going. Session ends `partial`, not `failed`. Lore exposes the aggregated note only when opening that `SessionDetail`; list/search summaries stay sparse.
2. **Version-keyed behavior via fixtures.** Each adapter carries fixtures per observed agent version (`docs/agents/<AGENT>.md` lists which). Parsing branches on stamped version only when necessary.
3. **Tolerate partial files.** A truncated final JSONL line (agent still writing) is expected; parse up to the last complete line and re-ingest on the next FSEvents change.
4. **No cross-adapter coupling.** An adapter error affects only its source job. `catch_unwind` is a last-resort guard for unwind-safe code, **not a sandbox** and not protection from abort/OOM/deadlock; size/depth/time/output bounds and cancellable jobs provide the actual resource boundary.
5. **Opaque fields stay opaque.** `encrypted_regions` (Codex `encrypted_content`) are stored only as `opaque_excluded` blobs if fidelity requires it and are **never decoded, rendered, indexed, or exported**.
6. **Preserve ordered blocks and changing context.** Emit Message + ordered MessageParts and start a SessionSegment whenever cwd/repo/model context changes; do not flatten mixed content into one `Message.kind`.
7. **Idempotency keys are mandatory.** Every upsert carries native/source identity sufficient for replay; archive moves, append resumes, and rewrites follow `DATA_MODEL.md` generation/checkpoint rules.

## 6. Registration

```rust
let mut registry = AdapterRegistry::new();
registry.register(Box::new(ClaudeCodeAdapter::default())); // V0
registry.register(Box::new(CodexAdapter::default()));      // V0
// V0.5+: GeminiCliAdapter, OpenCodeAdapter, SpecStoryMarkdownAdapter (fallback)
// V1:   CursorAdapter (experimental, isolated)
```

`DiscoveryRoots` is injectable so tests point adapters at fixture dirs instead of real `~/.claude`/`~/.codex` (see `TESTING.md`).

## 7. The SpecStory fallback adapter (V0.5, OPINION)

Because SpecStory writes normalized Markdown to `.specstory/history/` for *many* agents, a `SpecStoryMarkdownAdapter` gives Lore cheap breadth for agents we don't natively parse. Native adapters take precedence; the Markdown source is linked as an alternate (`dedupe_key`) to avoid double-counting the same conversation (see DATA_MODEL open question).

## 8. What adapters must NOT do
- Touch the network. Mutate agent files. Know about SQLite/FTS/UI. Depend on another adapter. Log raw secrets. Load an entire session into memory when streaming is possible.
