-- Content identity for recorded file changes, so a later pass can ask whether
-- an agent's work reached a commit.
--
-- `content_oid` is the **Git blob object id** of the file content the agent
-- produced — sha1("blob <len>\0" || bytes), the same address Git itself would
-- assign. Git hashes content, not commits, so an oid recorded here still
-- matches after a rebase, squash, cherry-pick or amend: the commit changes, the
-- blob does not. That property is the whole reason to store an oid rather than
-- a commit reference.
--
-- It is populated only where the resulting content is genuinely known — today
-- that means `create` events, whose recorded payload IS the new file. `edit`
-- events record a diff (Codex) or an old/new fragment (Claude Code), neither of
-- which yields the post-edit file without the pre-image, so their `content_oid`
-- stays NULL. NULL means "not assessed", never "did not land"; the query side
-- must preserve that distinction.
--
-- `content_oid_algo` records how the address was produced, mirroring
-- `blob.hash_algo` (migration 0010): the value is only comparable against a
-- repository that uses the same object format, and sha256 repositories exist.
ALTER TABLE file_event ADD COLUMN content_oid TEXT;
ALTER TABLE file_event ADD COLUMN content_oid_algo TEXT;

-- Landing lookups scan for events that have an oid and are attached to a
-- session, so index that pair rather than the oid alone.
CREATE INDEX ix_fileevent_content_oid ON file_event (content_oid, session_id)
    WHERE content_oid IS NOT NULL;
