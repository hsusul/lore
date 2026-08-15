-- Migration 0008: user-defined folders for organizing threads (sessions).
-- Lore-owned organizational metadata: created by the user, preserved across
-- archive re-scans, and never derived from agent files. One folder per session
-- (mutually exclusive), so filing a thread replaces any prior membership.

PRAGMA foreign_keys = ON;

CREATE TABLE folder (
    id         TEXT PRIMARY KEY,          -- random 128-bit hex
    name       TEXT NOT NULL,
    position   INTEGER NOT NULL DEFAULT 0, -- user ordering; ties break by name
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- One folder per session: session_id is the PRIMARY KEY, so re-filing a thread
-- replaces its membership via upsert. Rows are removed when the session is
-- forgotten or the folder is deleted.
CREATE TABLE session_folder (
    session_id TEXT NOT NULL PRIMARY KEY REFERENCES agent_session(id) ON DELETE CASCADE,
    folder_id  TEXT NOT NULL REFERENCES folder(id) ON DELETE CASCADE,
    added_at   INTEGER NOT NULL
) STRICT;
CREATE INDEX ix_session_folder_folder ON session_folder (folder_id, session_id);
