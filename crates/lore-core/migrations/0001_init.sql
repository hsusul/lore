-- Migration 0001: durable infrastructure tables (settings + job queue).
-- The full V0 entity schema (DATA_MODEL.md) is introduced in migration 0002+.
-- Append-only: never edit this file after it has shipped; add a new migration.

PRAGMA foreign_keys = ON;

-- Key/value application settings. Values are JSON text.
CREATE TABLE setting (
    key        TEXT    PRIMARY KEY,
    value_json TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- Durable background-job queue. Survives restarts so a crash resumes work
-- rather than restarting it. `state`: pending | running | done | failed.
CREATE TABLE job (
    id           TEXT    PRIMARY KEY,
    kind         TEXT    NOT NULL,
    priority     INTEGER NOT NULL DEFAULT 0,
    state        TEXT    NOT NULL DEFAULT 'pending',
    attempts     INTEGER NOT NULL DEFAULT 0,
    payload_json TEXT,
    error        TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
) STRICT;

-- Claim order: highest priority, then oldest, among runnable jobs.
CREATE INDEX job_runnable ON job (state, priority DESC, created_at);
