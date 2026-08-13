-- Migration 0005: source_artifact lookup indexes for ingest dedupe.
-- `ingest_file` probes an existing artifact by (agent_id, current_path) and by
-- (agent_id, native_file_id, full_hash); without an index each probe was a
-- growing full-table scan, making re-ingest super-linear in archive size.
-- Append-only: never edit this file after it has shipped.

CREATE INDEX ix_source_artifact_agent_path ON source_artifact (agent_id, current_path);
CREATE INDEX ix_source_artifact_agent_native_hash
    ON source_artifact (agent_id, native_file_id, full_hash);