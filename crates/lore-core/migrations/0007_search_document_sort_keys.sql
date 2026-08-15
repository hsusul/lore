-- Migration 0007: denormalize session sort/filter keys onto search_document.
-- The ranked search page sorts by session start time and filters by agent.
-- Carrying `started_at` and `agent_id` on the projection lets the page be
-- ordered and LIMITed on (search_fts + search_document) alone, joining
-- agent_session only for the handful of displayed rows instead of for every
-- match. This cuts the worst-case common-term query ~40% at scale
-- (SEARCH.md §6). Both keys are session-stable, so the projection never drifts
-- from agent_session between re-ingests. Backfill is a one-time pass.
-- Append-only: never edit this file after it has shipped.

ALTER TABLE search_document ADD COLUMN started_at INTEGER;
ALTER TABLE search_document ADD COLUMN agent_id TEXT;

UPDATE search_document
SET started_at = (SELECT started_at FROM agent_session WHERE id = search_document.session_id),
    agent_id   = (SELECT agent_id   FROM agent_session WHERE id = search_document.session_id);
