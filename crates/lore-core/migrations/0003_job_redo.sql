-- Migration 0003: coalescing support for the durable job queue.
-- A source whose file changes again while its ingest job is already running sets
-- redo = 1 so the in-flight run is re-scheduled on completion instead of the new
-- change being lost. Append-only: never edit this file after it has shipped.

ALTER TABLE job ADD COLUMN redo INTEGER NOT NULL DEFAULT 0;
