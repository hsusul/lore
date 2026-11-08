// Typed IPC surface over the Tauri commands/events. The payload types are the
// generated contract from `crates/lore-ipc/bindings` (never hand-edited); this
// module only names the commands and wires argument/return types to them.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

import type { DetectedAgent } from "../crates/lore-ipc/bindings/DetectedAgent";
import type { FileEventDto } from "../crates/lore-ipc/bindings/FileEventDto";
import type { FolderSummary } from "../crates/lore-ipc/bindings/FolderSummary";
import type { ForgetReport } from "../crates/lore-ipc/bindings/ForgetReport";
import type { GitObservationDto } from "../crates/lore-ipc/bindings/GitObservationDto";
import type { MessageDto } from "../crates/lore-ipc/bindings/MessageDto";
import type { MessagePage } from "../crates/lore-ipc/bindings/MessagePage";
import type { MessagePartDto } from "../crates/lore-ipc/bindings/MessagePartDto";
import type { RepositorySummary } from "../crates/lore-ipc/bindings/RepositorySummary";
import type { RescanResult } from "../crates/lore-ipc/bindings/RescanResult";
import type { ScanProgress } from "../crates/lore-ipc/bindings/ScanProgress";
import type { BackupScheduleDto } from "../crates/lore-ipc/bindings/BackupScheduleDto";
import type { CheckUpdateResultDto } from "../crates/lore-ipc/bindings/CheckUpdateResultDto";
import type { SearchHit } from "../crates/lore-ipc/bindings/SearchHit";
import type { SearchPage } from "../crates/lore-ipc/bindings/SearchPage";
import type { SegmentDto } from "../crates/lore-ipc/bindings/SegmentDto";
import type { SessionDetail } from "../crates/lore-ipc/bindings/SessionDetail";
import type { SessionPage } from "../crates/lore-ipc/bindings/SessionPage";
import type { SessionSummary } from "../crates/lore-ipc/bindings/SessionSummary";
import type { TokenTotalsDto } from "../crates/lore-ipc/bindings/TokenTotalsDto";
import type { LessonAgentDto } from "../crates/lore-ipc/bindings/LessonAgentDto";
import type { AutoLessonsDto } from "../crates/lore-ipc/bindings/AutoLessonsDto";
import type { LessonDto } from "../crates/lore-ipc/bindings/LessonDto";
import type { ReceiptDto } from "../crates/lore-ipc/bindings/ReceiptDto";
import type { NoteDto } from "../crates/lore-ipc/bindings/NoteDto";
import type { EnforceRuleDto } from "../crates/lore-ipc/bindings/EnforceRuleDto";
import type { DroppedRuleDto } from "../crates/lore-ipc/bindings/DroppedRuleDto";
import type { ProposalDto } from "../crates/lore-ipc/bindings/ProposalDto";
import type { ReadNoteDto } from "../crates/lore-ipc/bindings/ReadNoteDto";
import type { AgentsReadDto } from "../crates/lore-ipc/bindings/AgentsReadDto";
import type { SkillsReportDto } from "../crates/lore-ipc/bindings/SkillsReportDto";
import type { SkillDraftDto } from "../crates/lore-ipc/bindings/SkillDraftDto";
import type { SkillUsageDto } from "../crates/lore-ipc/bindings/SkillUsageDto";
import type { SkillFindingDto } from "../crates/lore-ipc/bindings/SkillFindingDto";
import type { SkillFolderDto } from "../crates/lore-ipc/bindings/SkillFolderDto";

export type BackupInterval = "off" | "daily" | "weekly";
export type SearchSort = "relevance" | "newest" | "oldest";
export type SearchSortOrder = SearchSort;
export type ParseStatus = "ok" | "partial" | "failed";
export type IdentityConfidence = "confirmed" | "high" | "medium" | "low";
export type GitTemporalConfidence =
  | "exact_event"
  | "near_event"
  | "retrospective"
  | "current_only";
export type FileChangeKind =
  | "edit"
  | "write"
  | "create"
  | "delete"
  | "move"
  | "read"
  | "patch";
export type MessageRole = "user" | "assistant" | "system" | "tool" | "meta";
export type MessagePartKind =
  | "text"
  | "thinking"
  | "tool_use"
  | "tool_result"
  | "attachment"
  | "summary"
  | "opaque"
  | "other";
export type MessageEventKind =
  | "message"
  | "summary"
  | "compaction"
  | "attachment"
  | "title"
  | "mode"
  | "pr_link"
  | "other";
export type FileEventSource = "agent_patch" | "agent_tool_input" | "lore_capture";
export type GitObservationProvenance =
  | "agent_recorded"
  | "lore_captured"
  | "lore_reflog"
  | "lore_reverified";
export type ResolutionConfidence = "high" | "medium" | "low" | "unresolved";
export type SecretScanState = "clean" | "redacted" | "failed_quarantined";
export type SecretSeverity = "low" | "medium" | "high" | "critical";
export type SearchSourceKind = "message_part" | "file_event" | "session";
export type SearchHitField = "title" | "text" | "patch" | "content_json";

export type {
  BackupScheduleDto,
  CheckUpdateResultDto,
  DetectedAgent,
  FileEventDto,
  FolderSummary,
  ForgetReport,
  GitObservationDto,
  MessageDto,
  MessagePage,
  MessagePartDto,
  RepositorySummary,
  RescanResult,
  ScanProgress,
  SearchHit,
  SearchPage,
  SegmentDto,
  SessionDetail,
  SessionPage,
  SessionSummary,
  TokenTotalsDto,
  LessonAgentDto,
  AutoLessonsDto,
  LessonDto,
  ReceiptDto,
  NoteDto,
  EnforceRuleDto,
  DroppedRuleDto,
  ProposalDto,
  ReadNoteDto,
  AgentsReadDto,
  SkillsReportDto,
  SkillDraftDto,
  SkillUsageDto,
  SkillFindingDto,
  SkillFolderDto,
};

/** Snippet highlight markers (must match lore_core::search). */
export const HIGHLIGHT_START = "\u{e000}";
export const HIGHLIGHT_END = "\u{e001}";

/** The agents Lore knows about, with ingested-session counts. */
export function listDetectedAgents(): Promise<DetectedAgent[]> {
  return invoke<DetectedAgent[]>("list_detected_agents");
}

/** Open the OS folder picker. No filesystem API is exposed to the webview. */
export function chooseAgentRootDirectory(displayName: string): Promise<string | null> {
  return open({
    directory: true,
    multiple: false,
    title: `Choose ${displayName} session folder`,
  });
}

/** Add a user-selected read-only source folder and scan it in the background. */
export function addAgentRoot(agentId: string, path: string): Promise<void> {
  return invoke<void>("add_agent_root", { agentId, path });
}

/** Stop scanning a user-selected folder; archived sessions remain intact. */
export function removeAgentRoot(agentId: string, path: string): Promise<void> {
  return invoke<void>("remove_agent_root", { agentId, path });
}

/** The most recent sessions, newest first, capped at `limit`. */
export function listSessions(limit: number = 50): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_sessions", { limit });
}

/** One newest-first session page. Pass the returned cursor back unchanged. */
export function listSessionsPage(
  limit: number = 50,
  cursor: string | null = null,
): Promise<SessionPage> {
  return invoke<SessionPage>("list_sessions_page", { limit, cursor });
}

/** The repositories resolved by git enrichment. */
export function listRepositories(): Promise<RepositorySummary[]> {
  return invoke<RepositorySummary[]>("list_repositories");
}

/** The most recent sessions that touched a repository, capped at `limit`. */
export function listRepositorySessions(
  id: string,
  limit: number = 50,
): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_repository_sessions", { id, limit });
}

/** One newest-first page of sessions that touched a repository. */
export function listRepositorySessionsPage(
  id: string,
  limit: number = 50,
  cursor: string | null = null,
): Promise<SessionPage> {
  return invoke<SessionPage>("list_repository_sessions_page", { id, limit, cursor });
}

/** Read one session in context, or null when it is unknown. */
export function getSession(id: string): Promise<SessionDetail | null> {
  return invoke<SessionDetail | null>("get_session", { id });
}

/** List one stable page of messages for a session. */
export function listSessionMessagesPage(
  id: string,
  limit: number = 200,
  cursor: string | null = null,
): Promise<MessagePage> {
  return invoke<MessagePage>("list_session_messages_page", { id, limit, cursor });
}

/** The user-defined folders, with thread counts. */
export function listFolders(): Promise<FolderSummary[]> {
  return invoke<FolderSummary[]>("list_folders");
}

/** Create a folder and return it (the name is trimmed and length-capped). */
export function createFolder(name: string): Promise<FolderSummary> {
  return invoke<FolderSummary>("create_folder", { name });
}

/** Rename a folder. */
export function renameFolder(id: string, name: string): Promise<void> {
  return invoke<void>("rename_folder", { id, name });
}

/** Delete a folder; its threads become unfiled but are not removed. */
export function deleteFolder(id: string): Promise<void> {
  return invoke<void>("delete_folder", { id });
}

/** File a thread into a folder, or unfile it when `folderId` is null. */
export function setSessionFolder(sessionId: string, folderId: string | null): Promise<void> {
  return invoke<void>("set_session_folder", { sessionId, folderId });
}

/** One newest-first page of the threads filed in a folder. */
export function listFolderSessionsPage(
  id: string,
  limit: number = 50,
  cursor: string | null = null,
): Promise<SessionPage> {
  return invoke<SessionPage>("list_folder_sessions_page", { id, limit, cursor });
}

/** Read the provenance-labeled git observations for a session. */
export function getGitSnapshot(id: string): Promise<GitObservationDto[]> {
  return invoke<GitObservationDto[]>("get_git_snapshot", { id });
}

/** Fetch the recorded patch text for a file event, or null when none is stored. */
export function getFilePatch(id: string): Promise<string | null> {
  return invoke<string | null>("get_file_patch", { id });
}

/** Full-text search over the redacted projections. Secret-safe by construction. */
export function search(query: string, limit: number = 50): Promise<SearchHit[]> {
  return invoke<SearchHit[]>("search", { query, limit });
}

/**
 * Paginated full-text search. Pass `cursor = null` for the first page; on each
 * result, if `next_cursor` is non-null, pass it back verbatim for the next page.
 * A cursor is valid only for the identical query and sort that produced it.
 * Keyset-based, so paging never drops or repeats a result.
 */
export function searchPage(
  query: string,
  limit: number = 50,
  cursor: string | null = null,
  sort: SearchSort = "relevance",
): Promise<SearchPage> {
  return invoke<SearchPage>("search_page", { query, limit, cursor, sort });
}

/** How many secrets were flagged in a session (all redacted from derived surfaces). */
export function sessionSecretCount(id: string): Promise<number> {
  return invoke<number>("session_secret_count", { id });
}

/** Export a session as Markdown; `includeSecrets` (default false) masks flagged spans. */
export function exportSessionMarkdown(
  id: string,
  includeSecrets: boolean = false,
): Promise<string | null> {
  return invoke<string | null>("export_session_markdown", {
    id,
    includeSecrets,
  });
}

/**
 * Write a session's redacted Markdown export. If `path` is omitted/null,
 * Lore opens the native save dialog in Rust. Returns true if saved, false if cancelled.
 */
export function saveSessionExport(
  id: string,
  path: string | null = null,
  includeSecrets: boolean = false,
): Promise<boolean> {
  return invoke<boolean>("save_session_export", { id, path, includeSecrets });
}

/** Forget a session: remove its rows, projections, findings, and orphan blobs. */
export function forgetSession(id: string): Promise<ForgetReport> {
  return invoke<ForgetReport>("forget_session", { id });
}

/** Forget everything: wipe all archive content (the database file stays). */
export function forgetEverything(): Promise<ForgetReport> {
  return invoke<ForgetReport>("forget_everything");
}

/** Run a discovery→ingest→enrich pass and resolve with the final tally. */
export function rescan(): Promise<RescanResult> {
  return invoke<RescanResult>("rescan");
}

/** Read a persisted setting's raw JSON value, or null when unset. */
export function getSetting(key: string): Promise<string | null> {
  return invoke<string | null>("get_setting", { key });
}

/** Read and parse a typed setting value, or null when unset or malformed. */
export async function getJsonSetting<T>(key: string): Promise<T | null> {
  const raw = await getSetting(key);
  if (!raw) return null;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

/** Persist a setting's raw JSON value (Lore-owned; cleared by "forget everything"). */
export function setSetting(key: string, valueJson: string): Promise<void> {
  return invoke<void>("set_setting", { key, valueJson });
}

/** Serialize and persist a typed setting value. */
export function setJsonSetting<T>(key: string, value: T): Promise<void> {
  const serialized = JSON.stringify(value);
  return setSetting(key, serialized ?? "null");
}

/** Read the automatic-backup schedule (interval + retention). */
export function getBackupSchedule(): Promise<BackupScheduleDto> {
  return invoke<BackupScheduleDto>("get_backup_schedule");
}

/** Persist the automatic-backup schedule. `interval` is "off" | "daily" | "weekly". */
export function setBackupSchedule(
  interval: BackupInterval,
  keep: number = 7,
): Promise<void> {
  return invoke<void>("set_backup_schedule", { interval, keep });
}

/** Create a Lore-owned backup now, pruning to the configured retention. */
export function backupNow(): Promise<void> {
  return invoke<void>("backup_now");
}

/** Subscribe to content-free scan progress. Resolves with an unlisten handle. */
export function onScanProgress(
  handler: (progress: ScanProgress) => void,
): Promise<UnlistenFn> {
  return listen<ScanProgress>("scan_progress", (event) => handler(event.payload));
}

/** Lessons mined from the user's corrections, waiting for review, most conversations first. */
export function listLessons(): Promise<LessonDto[]> {
  return invoke<LessonDto[]>("list_lessons");
}

/** Every active note in the vault, by path. Empty when there is no vault yet. */
export function listNotes(): Promise<NoteDto[]> {
  return invoke<NoteDto[]>("list_notes");
}

/** What an agent starting a session in `cwd` is given: the session-start notes section. */
export function agentsRead(cwd: string): Promise<AgentsReadDto> {
  return invoke<AgentsReadDto>("agents_read", { cwd });
}

/** What the skills doctor finds on this machine: findings worst first, with fixes, and skills per folder. */
export function skillsReport(): Promise<SkillsReportDto> {
  return invoke<SkillsReportDto>("skills_report");
}

/** Keep lesson `id` as a note, titled `title` or the suggested rule. Resolves with the note's vault path. */
export function acceptLesson(id: string, title?: string): Promise<string> {
  return invoke<string>("accept_lesson", { id, title: title ?? null });
}

/** Retire the note named by `key` (id, slug, or path) with `reason`. Resolves with the note's vault path. */
export function retireNote(key: string, reason: string): Promise<string> {
  return invoke<string>("retire_note", { key, reason });
}

/** Rewrite the note named by `key` with an edited title and body; its timeline is kept. Resolves with the note's vault path. */
export function editNote(key: string, title: string, body: string): Promise<string> {
  return invoke<string>("edit_note", { key, title, body });
}

/** How each installed skill has been used: uses, sessions, newest use, and the corrections right after it ran. Most reprompted first. */
export function skillUsage(): Promise<SkillUsageDto[]> {
  return invoke<SkillUsageDto[]>("skill_usage");
}

/** The lessons agents wrote and filed for review, oldest first. Empty when there is no vault yet. */
export function listProposals(): Promise<ProposalDto[]> {
  return invoke<ProposalDto[]>("list_proposals");
}

/** Accept proposal `id`, titled `title` when the person edited it. Resolves with the note's vault path. */
export function acceptProposal(id: string, title?: string): Promise<string> {
  return invoke<string>("accept_proposal", { id, title: title ?? null });
}

/** Reject proposal `id` with `reason`. */
export function rejectProposal(id: string, reason: string): Promise<void> {
  return invoke<void>("reject_proposal", { id, reason });
}

/** Run the person's `agent` (`claude-code` or `codex`) on the lore-lessons skill; resolves with its report once it finishes. */
export function findLessons(agent: string): Promise<string> {
  return invoke<string>("find_lessons", { agent });
}

/** Which agents can write lessons: Claude Code and Codex, each installed or not and signed in or not. */
export function lessonAgents(): Promise<LessonAgentDto[]> {
  return invoke<LessonAgentDto[]>("lesson_agents");
}

/** Whether Lore finds lessons on its own, and what its last automatic run did. */
export function getAutoLessons(): Promise<AutoLessonsDto> {
  return invoke<AutoLessonsDto>("get_auto_lessons");
}

/** Turn the automatic Find lessons run on or off; resolves with the new state. */
export function setAutoLessons(enabled: boolean): Promise<AutoLessonsDto> {
  return invoke<AutoLessonsDto>("set_auto_lessons", { enabled });
}

/** Subscribe to the automatic run starting and ending. */
export function onAutoLessons(handler: (state: AutoLessonsDto) => void): Promise<UnlistenFn> {
  return listen<AutoLessonsDto>("auto_lessons", (event) => handler(event.payload));
}

/** Ask `agent` to rewrite skill `name` from the corrections made after it ran. Writes nothing. */
export function draftSkillEdit(agent: string, name: string): Promise<SkillDraftDto> {
  return invoke<SkillDraftDto>("draft_skill_edit", { agent, name });
}

/** Write an approved draft of skill `name`, if its SKILL.md still reads `before`. Resolves with the backup's path. */
export function applySkillEdit(name: string, before: string, after: string): Promise<string> {
  return invoke<string>("apply_skill_edit", { name, before, after });
}

/** The person's name for the map's centre: the Mac account's full name, else the user name. */
export function accountName(): Promise<string | null> {
  return invoke<string | null>("account_name");
}

/** Calculate cumulative token usage and estimated cost across all sessions. */
export function getTokenTotals(): Promise<TokenTotalsDto> {
  return invoke<TokenTotalsDto>("get_token_totals");
}

/** Relink a session segment to a target repository (split/merge correction). */
export function relinkSegmentRepository(
  segmentId: string,
  repositoryId: string,
): Promise<void> {
  return invoke<void>("relink_segment_repository", { segmentId, repositoryId });
}

/** Check for application updates (explicit user action; ADR-0005). */
export function checkForUpdates(): Promise<CheckUpdateResultDto> {
  return invoke<CheckUpdateResultDto>("check_for_updates");
}
