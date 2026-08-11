// Typed IPC surface over the Tauri commands/events. The payload types are the
// generated contract from `crates/lore-ipc/bindings` (never hand-edited); this
// module only names the commands and wires argument/return types to them.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { DetectedAgent } from "../crates/lore-ipc/bindings/DetectedAgent";
import type { RescanResult } from "../crates/lore-ipc/bindings/RescanResult";
import type { ScanProgress } from "../crates/lore-ipc/bindings/ScanProgress";
import type { SessionSummary } from "../crates/lore-ipc/bindings/SessionSummary";

export type {
  DetectedAgent,
  RescanResult,
  ScanProgress,
  SessionSummary,
};

/** The agents Lore knows about, with ingested-session counts. */
export function listDetectedAgents(): Promise<DetectedAgent[]> {
  return invoke<DetectedAgent[]>("list_detected_agents");
}

/** The most recent sessions, newest first, capped at `limit`. */
export function listSessions(limit: number): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_sessions", { limit });
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
