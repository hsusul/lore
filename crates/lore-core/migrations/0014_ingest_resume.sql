-- Migration 0014: the parser checkpoint each source's last ingest ended at
-- (`ParseCheckpoint` JSON), so an append is parsed and persisted from there
-- instead of from the start of the file. NULL means no checkpoint: the next
-- change parses the whole file, as every change did before this migration.
ALTER TABLE ingest_state ADD COLUMN resume_json TEXT;
