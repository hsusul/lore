-- Migration 0010: record which digest produced each blob's content address.
--
-- Blob addresses were a 64-bit FNV-1a digest plus the byte length. FNV has no
-- collision resistance, and a blob address is load-bearing twice over: it is the
-- dedupe key (`stage` skips writing when the path already exists) and it carries
-- `scan_state`. Colliding content therefore inherited another blob's bytes and
-- its "clean" secret scan — a redaction-bypass path, not just a mix-up.
--
-- New writes use BLAKE3. Existing rows keep their fnv1a address and stay
-- readable (reads go through `storage_relpath`, which is unchanged); they are
-- re-addressed lazily when their source artifact is next re-ingested. This
-- column records which algorithm an address came from so the two can coexist
-- and so a later sweep can find the stragglers.
--
-- Append-only: never edit this file after it has shipped.

ALTER TABLE blob ADD COLUMN hash_algo TEXT NOT NULL DEFAULT 'fnv1a';
