-- Migration 0004: indexes declared in DATA_MODEL.md §8 as performance-critical
-- but missing from 0002. Lookups: repository identity resolution by kind + value
-- hash, and worktree scans by repository. Append-only: never edit this file
-- after it has shipped.

CREATE INDEX ix_repo_identity_kind_hash ON repository_identity_evidence (kind, value_hash);
CREATE INDEX ix_worktree_repository_path ON worktree (repository_id, path);