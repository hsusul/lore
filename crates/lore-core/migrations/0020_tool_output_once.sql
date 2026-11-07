-- Migration 0020: store each tool output once.
--
-- A tool_result part keeps the output in `text` (what search indexes and the
-- transcript shows) and again inside `content_json` (the raw payload). When
-- the payload holds that output exactly once, the payload is now stored with
-- the output replaced by a reserved marker, and message_part_full puts it
-- back from `text`:
--
--   char(1) || 'lore:text:quoted' || char(1)  stands for json_quote(text),
--       the output as a JSON string value (Claude Code results);
--   char(1) || 'lore:text:raw' || char(1)     stands for text verbatim, for a
--       payload that holds the output as raw JSON (Codex array outputs).
--
-- A raw char(1) byte never appears in JSON a parser accepts, so a payload
-- containing one is slim, and only slim payloads are rebuilt. A row whose
-- payload does not hold the output exactly once keeps both columns as before.
--
-- This migration only changes the read path and records how far existing rows
-- go: rows up to compact.tool_output_until are converted afterwards in
-- verified batches (`crate::compact`), and later ingests write the slim form.
DROP VIEW message_part_full;

CREATE VIEW message_part_full AS
SELECT
    p.id,
    p.message_id,
    p.ordinal,
    p.kind,
    p.text,
    CASE
        WHEN p.body_hash IS NOT NULL THEN b.body
        WHEN p.kind = 'tool_result' AND instr(p.content_json, char(1)) > 0 THEN
            CASE
                WHEN instr(p.content_json, char(1) || 'lore:text:quoted' || char(1)) > 0
                    THEN replace(p.content_json, char(1) || 'lore:text:quoted' || char(1),
                                 json_quote(p.text))
                WHEN instr(p.content_json, char(1) || 'lore:text:raw' || char(1)) > 0
                    THEN replace(p.content_json, char(1) || 'lore:text:raw' || char(1), p.text)
                ELSE p.content_json
            END
        ELSE p.content_json
    END AS content_json,
    p.blob_id,
    p.searchable,
    p.metadata_json,
    p.body_hash
FROM message_part p
LEFT JOIN part_body b ON b.hash = p.body_hash;

INSERT INTO setting (key, value_json, updated_at)
SELECT 'compact.tool_output_until', CAST(COALESCE(MAX(rowid), 0) AS TEXT),
       unixepoch('now') * 1000
FROM message_part
WHERE true
ON CONFLICT(key) DO NOTHING;
