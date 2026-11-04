-- Migration 0018: clear forgotten sessions' text out of the search index.
--
-- FTS5 records a delete as a tombstone and leaves the deleted row's terms in
-- its index segments until a merge reaches them. So the words of every
-- session forgotten so far are still in search_fts_data, in the archive
-- file, even after the VACUUM forget ran, which copies those segments as live
-- pages. Forget now merges the index after each delete (`crate::forget`).
--
-- This merges the index once, which drops what earlier forgets left, and
-- secure_delete zeroes the pages the old segments free.
PRAGMA secure_delete = ON;
INSERT INTO search_fts(search_fts) VALUES ('optimize');
