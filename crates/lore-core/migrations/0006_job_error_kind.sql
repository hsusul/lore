-- Migration 0006: stable, content-free job failure categories.
-- Human-readable diagnostics stay bounded in `error`; `error_kind` supports
-- reliable local observability without parsing SQLite error strings.

ALTER TABLE job ADD COLUMN error_kind TEXT;
