-- Migration 0017: index every foreign key's child columns.
--
-- SQLite looks up child rows when a parent row is deleted, and on every
-- parent insert while a deferred constraint is pending. Without an index each
-- lookup scanned the whole child table: every message_part insert scanned
-- tool_call, so a scan's time grew with the square of the archive's size.
CREATE INDEX ix_fileevent_patch_blob ON file_event (patch_blob_id);
CREATE INDEX ix_fileevent_tool_call ON file_event (tool_call_id);
CREATE INDEX ix_fileevent_segment ON file_event (segment_id);
CREATE INDEX ix_gitobs_diff_blob ON git_observation (diff_blob_id);
CREATE INDEX ix_gitobs_segment ON git_observation (segment_id);
CREATE INDEX ix_message_source_artifact ON message (source_artifact_id);
CREATE INDEX ix_message_segment ON message (segment_id);
CREATE INDEX ix_part_blob_id ON message_part (blob_id);
CREATE INDEX ix_repo_identity_repository ON repository_identity_evidence (repository_id);
CREATE INDEX ix_searchdoc_segment ON search_document (segment_id);
CREATE INDEX ix_search_git_segment ON search_git (segment_id);
CREATE INDEX ix_segment_worktree ON session_segment (worktree_id);
CREATE INDEX ix_toolcall_output_blob ON tool_call (output_blob_id);
CREATE INDEX ix_toolcall_input_blob ON tool_call (input_blob_id);
CREATE INDEX ix_toolcall_result_part ON tool_call (result_part_id);
CREATE INDEX ix_toolcall_call_part ON tool_call (call_part_id);
