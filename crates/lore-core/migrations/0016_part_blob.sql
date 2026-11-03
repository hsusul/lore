-- Migration 0016: which message parts reference which payload blobs.
--
-- Inline payloads (screenshots, attachments, PDFs) are moved out of a part's
-- content_json into the blob store and replaced by a `lore-blob:<hash>`
-- reference (`crate::payloads`). One part can hold several, and one blob can
-- be referenced by many parts, so the link is its own table. A part's links go
-- with it, so a forgotten session's payload blobs become orphans that forget
-- removes, while blobs other sessions still use stay.
CREATE TABLE part_blob (
    part_id TEXT NOT NULL REFERENCES message_part(id) ON DELETE CASCADE,
    blob_id TEXT NOT NULL REFERENCES blob(id),
    PRIMARY KEY (part_id, blob_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX ix_part_blob_blob ON part_blob (blob_id);
