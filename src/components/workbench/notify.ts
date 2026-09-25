// Tell the user about agents while Lore is not in front: OS notifications and
// the dock badge in the desktop app, the web Notification API in the browser
// preview. Everything here is best effort; a refusal just means no alert.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";

import type { TaskDto } from "../../ipc";
import { AGENT_LABELS } from "./state";

const IN_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export type AgentEvent = {
  id: string;
  title: string;
  kind: "finished" | "failed" | "attention" | "merged" | "handoff";
  message: string;
};

/**
 * What changed between two task lists that is worth telling the user. The
 * first list (null before) yields nothing, so launching Lore is quiet.
 */
export function taskEvents(previous: TaskDto[] | null, next: TaskDto[] | null): AgentEvent[] {
  if (!previous || !next) return [];
  const before = new Map(previous.map((t) => [t.id, t]));
  const events: AgentEvent[] = [];
  for (const task of next) {
    const was = before.get(task.id);
    if (!was) continue;
    const base = { id: task.id, title: task.title };
    if (!was.merged_into && task.merged_into) {
      events.push({ ...base, kind: "merged", message: `Merged into ${task.merged_into}` });
    } else if (!was.attention && task.attention) {
      events.push({ ...base, kind: "attention", message: task.attention });
    } else if (was.state === "running" && task.state === "failed") {
      events.push({
        ...base,
        kind: "failed",
        message: task.exit_code !== null ? `The agent exited with code ${task.exit_code}.` : "The run failed.",
      });
    } else if (was.state === "running" && task.state === "finished") {
      events.push({ ...base, kind: "finished", message: task.last_activity ?? "Finished." });
    } else if (was.agent !== task.agent && task.state === "running") {
      events.push({ ...base, kind: "handoff", message: `Handed off to ${AGENT_LABELS[task.agent]}.` });
    }
  }
  return events;
}

/** The notification's headline, e.g. "Fix parser needs you". */
export function eventHeadline(event: AgentEvent): string {
  const verb: Record<AgentEvent["kind"], string> = {
    finished: "finished",
    failed: "failed",
    attention: "needs you",
    merged: "merged",
    handoff: "was handed off",
  };
  return `${event.title} ${verb[event.kind]}`;
}

let permission: Promise<boolean> | null = null;

/** Ask once per session; the OS remembers the answer. */
function allowed(): Promise<boolean> {
  permission ??= (async () => {
    if (IN_TAURI) return (await isPermissionGranted()) || (await requestPermission()) === "granted";
    if (typeof Notification === "undefined" || Notification.permission === "denied") return false;
    return Notification.permission === "granted" || (await Notification.requestPermission()) === "granted";
  })().catch(() => false);
  return permission;
}

/** Whether the user is looking at Lore right now. */
export function appInFront(): boolean {
  return typeof document !== "undefined" && document.visibilityState === "visible" && document.hasFocus();
}

/** Post an OS notification (only call this while Lore is in the background). */
export async function notify(title: string, body: string): Promise<void> {
  if (!(await allowed())) return;
  try {
    if (IN_TAURI) sendNotification({ title, body });
    else new Notification(title, { body });
  } catch {
    // A notification that cannot be shown is not worth an error.
  }
}

/** The dock / taskbar badge: how many agents need the user. Zero clears it. */
export function setAttentionBadge(count: number): void {
  if (!IN_TAURI) return;
  getCurrentWindow()
    .setBadgeCount(count > 0 ? count : undefined)
    .catch(() => {});
}
