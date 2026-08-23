-- Migration 0012: fix the git-filter semi-join's branch:/commit: lookups.
--
-- Migration 0011 indexed branch and commit as (branch, source_class, session_id)
-- and (commit_sha, source_class, session_id). That order serves the
-- `git-source:`-constrained form, but a `branch:` (or `commit:`) filter WITHOUT a
-- source class correlates on `g.session_id = search_document.session_id` and must
-- then scan every source class for that branch/commit — one near-full scan per
-- candidate session. At 1M messages that made `add branch:main` take ~42 s.
--
-- These indexes put session_id immediately after the constant column, so the
-- correlated EXISTS seek is a single index lookup regardless of source class.
--
-- Append-only: never edit this file after it has shipped.

CREATE INDEX ix_search_git_branch_session ON search_git (branch, session_id);
CREATE INDEX ix_search_git_commit_session ON search_git (commit_sha, session_id);
