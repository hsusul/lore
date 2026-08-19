-- Migration 0009: indexes for three hot query paths that were doing full scans.
--
-- 1. `secret_count` (query.rs) filters secret_finding by session_id on every
--    session open; the only existing index leads with source_kind, so the
--    lookup scanned the whole table once per opened thread.
-- 2. The default browse order is `ORDER BY started_at DESC, id DESC`
--    (list_sessions / list_sessions_page); ix_session_agent_started leads with
--    agent_id, so every page paid a full scan plus a temp B-tree sort.
-- 3. session_source's PRIMARY KEY is (session_id, source_artifact_id), leaving
--    the ON DELETE CASCADE from source_artifact without an index on the
--    second column.
--
-- Append-only: never edit this file after it has shipped.

CREATE INDEX ix_secret_session ON secret_finding (session_id);
CREATE INDEX ix_session_started ON agent_session (started_at DESC, id DESC);
CREATE INDEX ix_session_source_artifact ON session_source (source_artifact_id);
