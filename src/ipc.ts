// Typed IPC surface over the Tauri commands/events. The payload types are the
// generated contract from `crates/lore-ipc/bindings` (never hand-edited); this
// module only names the commands and wires argument/return types to them.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { DetectedAgent } from "../crates/lore-ipc/bindings/DetectedAgent";
import type { FileEventDto } from "../crates/lore-ipc/bindings/FileEventDto";
import type { ForgetReport } from "../crates/lore-ipc/bindings/ForgetReport";
import type { GitObservationDto } from "../crates/lore-ipc/bindings/GitObservationDto";
import type { MessageDto } from "../crates/lore-ipc/bindings/MessageDto";
import type { MessagePartDto } from "../crates/lore-ipc/bindings/MessagePartDto";
import type { RepositorySummary } from "../crates/lore-ipc/bindings/RepositorySummary";
import type { RescanResult } from "../crates/lore-ipc/bindings/RescanResult";
import type { ScanProgress } from "../crates/lore-ipc/bindings/ScanProgress";
import type { SearchHit } from "../crates/lore-ipc/bindings/SearchHit";
import type { SegmentDto } from "../crates/lore-ipc/bindings/SegmentDto";
import type { SessionDetail } from "../crates/lore-ipc/bindings/SessionDetail";
import type { SessionSummary } from "../crates/lore-ipc/bindings/SessionSummary";

export type {
  DetectedAgent,
  FileEventDto,
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
  SessionSummary,
};

/** Snippet highlight markers (must match lore_core::search). */
export const HIGHLIGHT_START = "\u{e000}";
export const HIGHLIGHT_END = "\u{e001}";

/** The agents Lore knows about, with ingested-session counts. */
export function listDetectedAgents(): Promise<DetectedAgent[]> {
  return invoke<DetectedAgent[]>("list_detected_agents");
}

/** The most recent sessions, newest first, capped at `limit`. */
export function listSessions(limit: number): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_sessions", { limit });
}

/** The repositories resolved by git enrichment. */
export function listRepositories(): Promise<RepositorySummary[]> {
  return invoke<RepositorySummary[]>("list_repositories");
}

/** The most recent sessions that touched a repository, capped at `limit`. */
export function listRepositorySessions(
  id: string,
  limit: number,
): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_repository_sessions", { id, limit });
}

/** Read one session in context, or null when it is unknown. */
export function getSession(id: string): Promise<SessionDetail | null> {
  return invoke<SessionDetail | null>("get_session", { id });
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
export function search(query: string, limit: number): Promise<SearchHit[]> {
  return invoke<SearchHit[]>("search", { query, limit });
}

/** How many secrets were flagged in a session (all redacted from derived surfaces). */
export function sessionSecretCount(id: string): Promise<number> {
  return invoke<number>("session_secret_count", { id });
}

/** Export a session as Markdown; `includeSecrets` (default false) masks flagged spans. */
export function exportSessionMarkdown(
  id: string,
  includeSecrets: boolean,
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

/** Run a discovery→ingest→enrich pass and resolve with the final tally. */
export function rescan(): Promise<RescanResult> {
  return invoke<RescanResult>("rescan");
}

/** Subscribe to content-free scan progress. Resolves with an unlisten handle. */
export function onScanProgress(
  handler: (progress: ScanProgress) => void,
): Promise<UnlistenFn> {
  return listen<ScanProgress>("scan_progress", (event) => handler(event.payload));
}
