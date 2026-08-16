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
import type { MessagePartDto } from "../crates/lore-ipc/bindings/MessagePartDto";
import type { RepositorySummary } from "../crates/lore-ipc/bindings/RepositorySummary";
import type { RescanResult } from "../crates/lore-ipc/bindings/RescanResult";
import type { ScanProgress } from "../crates/lore-ipc/bindings/ScanProgress";
import type { BackupScheduleDto } from "../crates/lore-ipc/bindings/BackupScheduleDto";
import type { SearchHit } from "../crates/lore-ipc/bindings/SearchHit";
import type { SearchPage } from "../crates/lore-ipc/bindings/SearchPage";
import type { SegmentDto } from "../crates/lore-ipc/bindings/SegmentDto";
import type { SessionDetail } from "../crates/lore-ipc/bindings/SessionDetail";
import type { SessionPage } from "../crates/lore-ipc/bindings/SessionPage";
import type { SessionSummary } from "../crates/lore-ipc/bindings/SessionSummary";

export type {
  BackupScheduleDto,
  DetectedAgent,
  FileEventDto,
  FolderSummary,
  ForgetReport,
  GitObservationDto,
  MessageDto,
  MessagePartDto,
  RepositorySummary,
  RescanResult,
  ScanProgress,
  SearchHit,
  SegmentDto,
  SessionDetail,
  SessionPage,
  SessionSummary,
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

/** Result ordering for {@link searchPage}. */
export type SearchSort = "relevance" | "newest" | "oldest";

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

export type BackupInterval = "off" | "daily" | "weekly";

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

/** Persist a setting's raw JSON value (Lore-owned; cleared by "forget everything"). */
export function setSetting(key: string, valueJson: string): Promise<void> {
  return invoke<void>("set_setting", { key, valueJson });
}

/** Read the automatic-backup schedule (interval + retention). */
export function getBackupSchedule(): Promise<BackupScheduleDto> {
  return invoke<BackupScheduleDto>("get_backup_schedule");
}

/** Persist the automatic-backup schedule. `interval` is "off" | "daily" | "weekly". */
export function setBackupSchedule(
  interval: BackupInterval | string,
  keep: number,
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
