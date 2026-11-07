-- Migration 0019: store each attachment body once.
--
-- Claude Code injects the same context into many sessions, so most attachment
-- parts hold a body some other part already holds, byte for byte. Each
-- distinct body is now kept once in part_body, keyed by the BLAKE3 hash of its
-- exact bytes, and an attachment part points at it through body_hash with
-- content_json left NULL. Identical bodies collapse; different ones never do.
--
-- message_part_full is the one read path. It returns content_json exactly as
-- ingested whichever way the row is stored, so no reader can see the NULL a
-- deduplicated row leaves in message_part.content_json.
--
-- This migration only adds the shape. Existing rows are converted afterwards
-- in batches, each verified byte for byte before the old column is cleared
-- (`crate::compact`), so the conversion can stop and resume at any point.
CREATE TABLE part_body (
    hash TEXT PRIMARY KEY,
    body TEXT NOT NULL
) STRICT;

ALTER TABLE message_part ADD COLUMN body_hash TEXT REFERENCES part_body(hash);
CREATE INDEX ix_part_body_hash ON message_part (body_hash);

CREATE VIEW message_part_full AS
SELECT
    p.id,
    p.message_id,
    p.ordinal,
    p.kind,
    p.text,
    CASE WHEN p.body_hash IS NULL THEN p.content_json ELSE b.body END AS content_json,
    p.blob_id,
    p.searchable,
    p.metadata_json,
    p.body_hash
FROM message_part p
LEFT JOIN part_body b ON b.hash = p.body_hash;
