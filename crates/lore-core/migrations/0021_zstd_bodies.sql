-- Migration 0021: message bodies may be stored zstd-compressed.
--
-- Four columns may hold a compressed value: part_body.body,
-- message_part.text, message_part.content_json and tool_call.input_json.
-- search_document.redacted_text, the full-text index's content, never does.
--
-- A compressed value is a BLOB (`crate::codec`): a magic byte, the id of the
-- zstd_dict row it was compressed with, then a zstd frame with a checksum. A
-- plain value stays TEXT, so both coexist and the reader branches on the
-- value's type. The three tables were STRICT with TEXT columns, so they are
-- rebuilt with those columns ANY, keeping every rowid, constraint, index and
-- byte. The runner applies this migration with foreign keys off and runs
-- foreign_key_check before it commits, the procedure SQLite documents for
-- rebuilding a referenced table.
--
-- A dictionary is trained per column from the archive's own rows
-- (`crate::compact`) and never replaced once rows use it; a new dictionary
-- gets a new id. `until_rowid` is the table's last rowid when the dictionary
-- was stored: rows after it were written compressed by ingest, rows up to it
-- are converted by compaction in verified batches.
--
-- The views are the one place a value is decoded: lore_unpack (registered on
-- every archive connection) returns TEXT as it is and decompresses a BLOB
-- with the dictionary its header names. Every reader selects from
-- part_body_full, message_part_full or tool_call_full, so it still gets the
-- exact bytes that were ingested. The tool output marker (migration 0020) is
-- put back after decoding, from the decoded text.
CREATE TABLE zstd_dict (
    id          INTEGER PRIMARY KEY,
    "column"    TEXT    NOT NULL,
    dict        BLOB    NOT NULL,
    until_rowid INTEGER NOT NULL,
    trained_at  INTEGER NOT NULL
) STRICT;

DROP VIEW message_part_full;

CREATE TABLE part_body_new (
    hash TEXT PRIMARY KEY,
    body ANY  NOT NULL
) STRICT;
INSERT INTO part_body_new (rowid, hash, body) SELECT rowid, hash, body FROM part_body;
DROP TABLE part_body;
ALTER TABLE part_body_new RENAME TO part_body;

CREATE TABLE message_part_new (
    id            TEXT PRIMARY KEY,
    message_id    TEXT NOT NULL REFERENCES message(id) ON DELETE CASCADE,
    ordinal       INTEGER NOT NULL,
    kind          TEXT NOT NULL, -- text|thinking|tool_use|tool_result|attachment|summary|opaque|other
    text          ANY,
    content_json  ANY,
    blob_id       TEXT REFERENCES blob(id),
    searchable    INTEGER NOT NULL DEFAULT 1,
    metadata_json TEXT,
    body_hash     TEXT REFERENCES part_body(hash)
) STRICT;
INSERT INTO message_part_new
    (rowid, id, message_id, ordinal, kind, text, content_json, blob_id, searchable,
     metadata_json, body_hash)
SELECT rowid, id, message_id, ordinal, kind, text, content_json, blob_id, searchable,
       metadata_json, body_hash
FROM message_part;
DROP TABLE message_part;
ALTER TABLE message_part_new RENAME TO message_part;
CREATE UNIQUE INDEX ux_part_ordinal ON message_part (message_id, ordinal);
CREATE INDEX ix_part_blob_id ON message_part (blob_id);
CREATE INDEX ix_part_body_hash ON message_part (body_hash);

CREATE TABLE tool_call_new (
    id             TEXT PRIMARY KEY,
    session_id     TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    call_part_id   TEXT NOT NULL REFERENCES message_part(id),
    result_part_id TEXT REFERENCES message_part(id),
    native_call_id TEXT NOT NULL,
    name           TEXT NOT NULL,
    input_json     ANY,
    input_blob_id  TEXT REFERENCES blob(id),
    output_text    TEXT,
    output_blob_id TEXT REFERENCES blob(id),
    is_error       INTEGER,
    duration_ms    INTEGER
) STRICT;
INSERT INTO tool_call_new
    (rowid, id, session_id, call_part_id, result_part_id, native_call_id, name, input_json,
     input_blob_id, output_text, output_blob_id, is_error, duration_ms)
SELECT rowid, id, session_id, call_part_id, result_part_id, native_call_id, name, input_json,
       input_blob_id, output_text, output_blob_id, is_error, duration_ms
FROM tool_call;
DROP TABLE tool_call;
ALTER TABLE tool_call_new RENAME TO tool_call;
CREATE UNIQUE INDEX ux_toolcall_native ON tool_call (session_id, native_call_id);
CREATE INDEX ix_toolcall_output_blob ON tool_call (output_blob_id);
CREATE INDEX ix_toolcall_input_blob ON tool_call (input_blob_id);
CREATE INDEX ix_toolcall_result_part ON tool_call (result_part_id);
CREATE INDEX ix_toolcall_call_part ON tool_call (call_part_id);

CREATE VIEW part_body_full AS
SELECT
    hash,
    lore_unpack(body, (SELECT d.dict FROM zstd_dict d WHERE d.id = lore_dict_id(body))) AS body
FROM part_body;

CREATE VIEW tool_call_full AS
SELECT
    id,
    session_id,
    call_part_id,
    result_part_id,
    native_call_id,
    name,
    lore_unpack(input_json,
                (SELECT d.dict FROM zstd_dict d WHERE d.id = lore_dict_id(input_json)))
        AS input_json,
    input_blob_id,
    output_text,
    output_blob_id,
    is_error,
    duration_ms
FROM tool_call;

-- Decoded columns first, then the tool output marker on the decoded values.
CREATE VIEW message_part_unpacked AS
SELECT
    id,
    message_id,
    ordinal,
    kind,
    lore_unpack(text, (SELECT d.dict FROM zstd_dict d WHERE d.id = lore_dict_id(text)))
        AS text,
    lore_unpack(content_json,
                (SELECT d.dict FROM zstd_dict d WHERE d.id = lore_dict_id(content_json)))
        AS content_json,
    blob_id,
    searchable,
    metadata_json,
    body_hash
FROM message_part;

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
FROM message_part_unpacked p
LEFT JOIN part_body_full b ON b.hash = p.body_hash;
