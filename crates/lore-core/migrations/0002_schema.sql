-- Migration 0002: full V0 normalized schema (docs/architecture/DATA_MODEL.md).
-- Append-only; never edit after shipping. Tables are created in FK-safe order.
-- Conventions: ids are ULID TEXT unless noted; timestamps are UTC epoch ms
-- stored as INTEGER; booleans are INTEGER 0/1; JSON is TEXT. STRICT everywhere
-- except the FTS5 virtual table.

PRAGMA foreign_keys = ON;

-- ── Source and agent ────────────────────────────────────────────────────────

CREATE TABLE agent (
    id                TEXT PRIMARY KEY,          -- 'claude-code', 'codex', ...
    display_name      TEXT NOT NULL,
    detected          INTEGER NOT NULL DEFAULT 0,
    version           TEXT,
    capabilities_json TEXT
) STRICT;

-- Content-addressed large payloads. scan_state gates search/export.
CREATE TABLE blob (
    id             TEXT PRIMARY KEY,
    content_hash   TEXT NOT NULL UNIQUE,
    media_type     TEXT NOT NULL,
    byte_len       INTEGER NOT NULL,
    storage_relpath TEXT NOT NULL,
    compression    TEXT,
    scan_state     TEXT NOT NULL DEFAULT 'pending', -- pending|clean|findings|opaque_excluded|failed_quarantined
    created_at     INTEGER NOT NULL
) STRICT;

CREATE TABLE source_artifact (
    id             TEXT PRIMARY KEY,
    agent_id       TEXT NOT NULL REFERENCES agent(id),
    current_path   TEXT NOT NULL,
    native_file_id TEXT,
    size           INTEGER,
    mtime          INTEGER,
    full_hash      TEXT,
    prefix_hash    TEXT,
    generation     INTEGER NOT NULL DEFAULT 0,
    state          TEXT NOT NULL DEFAULT 'active', -- active|archived|missing|quarantined
    first_seen_at  INTEGER NOT NULL,
    last_seen_at   INTEGER NOT NULL
) STRICT;

-- Retains move/archive path aliases; path alone never defines identity.
CREATE TABLE source_artifact_path (
    source_artifact_id TEXT NOT NULL REFERENCES source_artifact(id) ON DELETE CASCADE,
    path               TEXT NOT NULL,
    first_seen_at      INTEGER NOT NULL,
    last_seen_at       INTEGER NOT NULL,
    PRIMARY KEY (source_artifact_id, path)
) STRICT;

-- Committed parser checkpoint; resume only after generation/size/prefix proof.
CREATE TABLE ingest_state (
    source_artifact_id TEXT PRIMARY KEY REFERENCES source_artifact(id) ON DELETE CASCADE,
    generation         INTEGER NOT NULL,
    last_offset        INTEGER NOT NULL DEFAULT 0,
    last_line          INTEGER NOT NULL DEFAULT 0,
    prefix_hash        TEXT,
    parser_version     TEXT NOT NULL,
    state              TEXT NOT NULL,
    updated_at         INTEGER NOT NULL
) STRICT;

-- ── Repository and git identity ─────────────────────────────────────────────

CREATE TABLE repository (
    id                  TEXT PRIMARY KEY,
    identity_key        TEXT NOT NULL UNIQUE,     -- opaque Lore identity; not a root SHA/path
    display_name        TEXT NOT NULL,
    primary_path        TEXT,
    identity_confidence TEXT NOT NULL DEFAULT 'low', -- confirmed|high|medium|low
    is_missing          INTEGER NOT NULL DEFAULT 0,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
) STRICT;

CREATE TABLE repository_identity_evidence (
    id            TEXT PRIMARY KEY,
    repository_id TEXT NOT NULL REFERENCES repository(id) ON DELETE CASCADE,
    kind          TEXT NOT NULL, -- git_common_dir|remote_root|remote|root_set|path|user_merge|user_split
    value_hash    TEXT NOT NULL,
    display_value TEXT,
    confidence    TEXT NOT NULL,
    first_seen_at INTEGER NOT NULL,
    last_seen_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE worktree (
    id                  TEXT PRIMARY KEY,
    repository_id       TEXT NOT NULL REFERENCES repository(id) ON DELETE CASCADE,
    path                TEXT NOT NULL,
    git_common_dir_hash TEXT NOT NULL,
    branch_hint         TEXT,
    is_primary          INTEGER NOT NULL DEFAULT 0,
    is_missing          INTEGER NOT NULL DEFAULT 0
) STRICT;

-- ── Sessions ────────────────────────────────────────────────────────────────

CREATE TABLE agent_session (
    id                TEXT PRIMARY KEY,
    agent_id          TEXT NOT NULL REFERENCES agent(id),
    native_session_id TEXT,
    dedupe_key        TEXT NOT NULL,
    title             TEXT,
    started_at        INTEGER,
    ended_at          INTEGER,
    primary_model     TEXT,
    message_count     INTEGER NOT NULL DEFAULT 0,
    tool_call_count   INTEGER NOT NULL DEFAULT 0,
    total_input_tokens  INTEGER,
    total_output_tokens INTEGER,
    total_cache_tokens  INTEGER,
    est_cost_usd      REAL,
    parse_status      TEXT NOT NULL DEFAULT 'ok', -- ok|partial|failed
    parse_note        TEXT,
    agent_version     TEXT,
    metadata_json     TEXT
) STRICT;

-- Uniqueness: native id when present, else deterministic dedupe key.
CREATE UNIQUE INDEX ux_session_native
    ON agent_session (agent_id, native_session_id) WHERE native_session_id IS NOT NULL;
CREATE UNIQUE INDEX ux_session_dedupe
    ON agent_session (agent_id, dedupe_key) WHERE native_session_id IS NULL;
CREATE INDEX ix_session_agent_started ON agent_session (agent_id, started_at DESC);

-- A logical session may draw from several source artifacts (copy/continue/archive).
CREATE TABLE session_source (
    session_id         TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    source_artifact_id TEXT NOT NULL REFERENCES source_artifact(id) ON DELETE CASCADE,
    first_seq          INTEGER,
    last_seq           INTEGER,
    PRIMARY KEY (session_id, source_artifact_id)
) STRICT;

-- Context valid for an inclusive event range; new segment when context changes.
CREATE TABLE session_segment (
    id                    TEXT PRIMARY KEY,
    session_id            TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    seq_start             INTEGER NOT NULL,
    seq_end               INTEGER NOT NULL,
    cwd                   TEXT,
    repository_id         TEXT REFERENCES repository(id),
    worktree_id           TEXT REFERENCES worktree(id),
    model                 TEXT,
    provider              TEXT,
    context_source        TEXT NOT NULL DEFAULT 'event', -- event|session_header|inferred
    resolution_confidence TEXT NOT NULL DEFAULT 'unresolved' -- high|medium|low|unresolved
) STRICT;
CREATE INDEX ix_segment_session ON session_segment (session_id, seq_start);
CREATE INDEX ix_segment_repo ON session_segment (repository_id, session_id);

-- ── Messages, parts, tool calls, file events ────────────────────────────────

CREATE TABLE message (
    id                 TEXT PRIMARY KEY,
    session_id         TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    segment_id         TEXT REFERENCES session_segment(id),
    native_uuid        TEXT,
    parent_id          TEXT REFERENCES message(id),
    parent_native_uuid TEXT,
    seq                INTEGER NOT NULL,
    role               TEXT NOT NULL,       -- user|assistant|system|tool|meta
    event_kind         TEXT NOT NULL DEFAULT 'message', -- message|summary|compaction|attachment|title|mode|pr_link|other
    is_sidechain       INTEGER NOT NULL DEFAULT 0,
    ts                 INTEGER,
    model              TEXT,
    token_input        INTEGER,
    token_output       INTEGER,
    token_cache        INTEGER,
    stop_reason        TEXT,
    source_artifact_id TEXT REFERENCES source_artifact(id),
    source_offset      INTEGER,
    metadata_json      TEXT
) STRICT;
CREATE UNIQUE INDEX ux_message_seq ON message (session_id, seq);
CREATE UNIQUE INDEX ux_message_uuid ON message (session_id, native_uuid) WHERE native_uuid IS NOT NULL;
CREATE INDEX ix_message_parent ON message (parent_id);

CREATE TABLE message_part (
    id            TEXT PRIMARY KEY,
    message_id    TEXT NOT NULL REFERENCES message(id) ON DELETE CASCADE,
    ordinal       INTEGER NOT NULL,
    kind          TEXT NOT NULL, -- text|thinking|tool_use|tool_result|attachment|summary|opaque|other
    text          TEXT,
    content_json  TEXT,
    blob_id       TEXT REFERENCES blob(id),
    searchable    INTEGER NOT NULL DEFAULT 1,
    metadata_json TEXT
) STRICT;
CREATE UNIQUE INDEX ux_part_ordinal ON message_part (message_id, ordinal);

CREATE TABLE tool_call (
    id             TEXT PRIMARY KEY,
    session_id     TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    call_part_id   TEXT NOT NULL REFERENCES message_part(id),
    result_part_id TEXT REFERENCES message_part(id),
    native_call_id TEXT NOT NULL,
    name           TEXT NOT NULL,
    input_json     TEXT,
    input_blob_id  TEXT REFERENCES blob(id),
    output_text    TEXT,
    output_blob_id TEXT REFERENCES blob(id),
    is_error       INTEGER,
    duration_ms    INTEGER
) STRICT;
CREATE UNIQUE INDEX ux_toolcall_native ON tool_call (session_id, native_call_id);

CREATE TABLE file_event (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    segment_id    TEXT REFERENCES session_segment(id),
    tool_call_id  TEXT REFERENCES tool_call(id),
    path          TEXT NOT NULL,
    change_kind   TEXT NOT NULL, -- edit|write|create|delete|move|read|patch
    old_path      TEXT,
    lines_added   INTEGER,
    lines_removed INTEGER,
    patch_blob_id TEXT REFERENCES blob(id),
    source        TEXT NOT NULL, -- agent_patch|agent_tool_input|lore_capture
    event_ts      INTEGER,
    observed_at   INTEGER
) STRICT;
CREATE INDEX ix_fileevent_path ON file_event (session_id, path);

-- ── Git observations (provenance-separated; never overwrite one another) ─────

CREATE TABLE git_observation (
    id                 TEXT PRIMARY KEY,
    session_id         TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    segment_id         TEXT REFERENCES session_segment(id),
    source             TEXT NOT NULL, -- agent_recorded|agent_patch|lore_captured|lore_reverified
    event_ts           INTEGER,
    observed_at        INTEGER NOT NULL,
    temporal_confidence TEXT NOT NULL, -- exact_event|near_event|retrospective|current_only
    branch             TEXT,
    commit_sha         TEXT,
    commit_subject     TEXT,
    remote_url_norm    TEXT,
    is_dirty           INTEGER,
    ahead              INTEGER,
    behind             INTEGER,
    changed_files_json TEXT,
    diff_blob_id       TEXT REFERENCES blob(id),
    commit_exists      INTEGER,
    metadata_json      TEXT
) STRICT;
CREATE INDEX ix_gitobs_session ON git_observation (session_id, source, observed_at);
CREATE INDEX ix_gitobs_commit ON git_observation (commit_sha);

-- ── Secrets and search ──────────────────────────────────────────────────────

-- Polymorphic target via (source_kind, source_id, field); no raw value stored.
CREATE TABLE secret_finding (
    id                TEXT PRIMARY KEY,
    session_id        TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    source_kind       TEXT NOT NULL,
    source_id         TEXT NOT NULL,
    field             TEXT NOT NULL,
    rule              TEXT NOT NULL,
    span_start        INTEGER NOT NULL,
    span_end          INTEGER NOT NULL,
    severity          TEXT NOT NULL,
    value_fingerprint TEXT,
    disposition       TEXT NOT NULL, -- redacted|allowlisted|revealed
    created_at        INTEGER NOT NULL
) STRICT;
CREATE INDEX ix_secret_target ON secret_finding (source_kind, source_id, field);

-- Sole external-content table for FTS5. redacted_text never holds flagged spans.
CREATE TABLE search_document (
    id           INTEGER PRIMARY KEY,   -- rowid alias for FTS5 content_rowid
    session_id   TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    segment_id   TEXT REFERENCES session_segment(id),
    source_kind  TEXT NOT NULL,
    source_id    TEXT NOT NULL,
    field        TEXT NOT NULL,
    ordinal      INTEGER NOT NULL DEFAULT 0,
    redacted_text TEXT NOT NULL,
    created_at   INTEGER NOT NULL
) STRICT;
CREATE INDEX ix_searchdoc_source ON search_document (session_id, source_kind, source_id);

CREATE VIRTUAL TABLE search_fts USING fts5(
    redacted_text,
    content = 'search_document',
    content_rowid = 'id'
);

-- Keep the FTS index in sync with its external content in the same transaction.
CREATE TRIGGER search_document_ai AFTER INSERT ON search_document BEGIN
    INSERT INTO search_fts(rowid, redacted_text) VALUES (new.id, new.redacted_text);
END;
CREATE TRIGGER search_document_ad AFTER DELETE ON search_document BEGIN
    INSERT INTO search_fts(search_fts, rowid, redacted_text) VALUES ('delete', old.id, old.redacted_text);
END;
CREATE TRIGGER search_document_au AFTER UPDATE ON search_document BEGIN
    INSERT INTO search_fts(search_fts, rowid, redacted_text) VALUES ('delete', old.id, old.redacted_text);
    INSERT INTO search_fts(rowid, redacted_text) VALUES (new.id, new.redacted_text);
END;
