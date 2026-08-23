-- Migration 0011: a provenance-preserving filter surface for git search.
--
-- Migration 0007 denormalized `started_at` and `agent_id` onto search_document
-- because both are scalar and session-stable: one session has exactly one of
-- each, so the projection can never drift. Git evidence is neither.
--
-- One session has N segments (cwd can change mid-conversation), each segment
-- carries M git_observation rows, and those rows span three source classes
-- (agent_recorded | lore_captured | lore_reverified) — after the append-on-
-- verdict-change change, a segment can hold several lore_reverified rows.
-- `agent_recorded.branch = billing` and `lore_captured.branch = main` are both
-- true of the same session. Collapsing them into one column on search_document
-- would flatten exactly the distinction GIT_INTEGRATION.md §1 exists to
-- preserve, and joining git_observation directly would fan out and count one
-- match many times.
--
-- So: a narrow filter table, queried by semi-join (EXISTS). `source_class` stays
-- a real dimension the query can constrain rather than a value that got merged,
-- and the ranked page is still computable on search_fts + search_document
-- alone — the property 0007 was protecting.
--
-- Derived data. Rebuildable from git_observation and session_segment without
-- reparsing agent logs (SEARCH.md §5), and written in the same transaction as
-- the canonical rows it projects.
--
-- Append-only: never edit this file after it has shipped.

CREATE TABLE search_git (
    session_id    TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    segment_id    TEXT REFERENCES session_segment(id),
    repository_id TEXT,
    worktree_id   TEXT,
    -- agent_recorded | lore_captured | lore_reverified
    source_class  TEXT NOT NULL,
    branch        TEXT,
    commit_sha    TEXT,
    observed_at   INTEGER NOT NULL
) STRICT;

-- No PRIMARY KEY: branch and commit_sha are nullable, and SQLite treats NULLs
-- as distinct in a UNIQUE index, so a natural key over them would neither
-- deduplicate nor constrain. Rows are rebuilt wholesale per session instead
-- (delete-then-insert, mirroring how search_document is maintained).
CREATE UNIQUE INDEX ux_search_git_row ON search_git (
    session_id,
    ifnull(segment_id, ''),
    source_class,
    ifnull(branch, ''),
    ifnull(commit_sha, '')
);

-- repo: and worktree: filters.
CREATE INDEX ix_search_git_repo ON search_git (repository_id, session_id);
CREATE INDEX ix_search_git_worktree ON search_git (worktree_id, session_id);
-- branch: filters, optionally constrained by git-source:.
CREATE INDEX ix_search_git_branch ON search_git (branch, source_class, session_id);
-- commit: filters. Commit lookup is the one case that is useful without a
-- source class, so the class trails the sha.
CREATE INDEX ix_search_git_commit ON search_git (commit_sha, source_class, session_id);

-- Backfill from the canonical evidence. A segment with no git_observation
-- still contributes its repository/worktree linkage, so `repo:` matches a
-- session whose repository was resolved even when no observation was recorded.
INSERT OR IGNORE INTO search_git
    (session_id, segment_id, repository_id, worktree_id, source_class, branch, commit_sha, observed_at)
SELECT o.session_id,
       o.segment_id,
       s.repository_id,
       s.worktree_id,
       o.source,
       o.branch,
       o.commit_sha,
       o.observed_at
FROM git_observation o
LEFT JOIN session_segment s ON s.id = o.segment_id;

INSERT OR IGNORE INTO search_git
    (session_id, segment_id, repository_id, worktree_id, source_class, branch, commit_sha, observed_at)
SELECT s.session_id,
       s.id,
       s.repository_id,
       s.worktree_id,
       'segment_link',
       NULL,
       NULL,
       0
FROM session_segment s
WHERE s.repository_id IS NOT NULL;
