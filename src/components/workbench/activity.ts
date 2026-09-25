// Turn an agent's flat activity list into what the timeline shows: runs with
// the prompt that started them, and output lines grouped into blocks.

import type { ActivityDto } from "../../ipc";

/** The orchestrator's log markers (`RUN_MARKER` in lore-orchestrator). */
const OPENING_PROMPT = /^--- Lore: opening prompt ---$/;
const RUN_HEADER = /^--- Lore: run (\d+) \((.+)\) ---$/;

export type TimelineEntry =
  | { kind: "run"; run: number; agent: string | null }
  | { kind: "prompt"; text: string }
  | { kind: "message" | "result" | "error"; text: string }
  | { kind: "tool"; tool: ToolCall }
  | { kind: "output"; lines: string[] };

export type ToolCall = {
  /** "Edit", "Bash", "run", … as the agent named it. */
  name: string;
  target: string;
  family: "read" | "edit" | "shell" | "search" | "web" | "other";
};

const FAMILIES: Record<string, ToolCall["family"]> = {
  read: "read",
  notebookread: "read",
  edit: "edit",
  multiedit: "edit",
  write: "edit",
  notebookedit: "edit",
  apply_patch: "edit",
  bash: "shell",
  run: "shell",
  shell: "shell",
  grep: "search",
  glob: "search",
  ls: "search",
  webfetch: "web",
  websearch: "web",
};

/** "Edit src/a.rs" → { name: "Edit", target: "src/a.rs", family: "edit" }. */
export function parseTool(text: string): ToolCall {
  const trimmed = text.trim();
  // Codex reports patches as "edit files" with no target.
  if (trimmed === "edit files") return { name: "Edit", target: "files", family: "edit" };
  const space = trimmed.indexOf(" ");
  const rawName = space < 0 ? trimmed : trimmed.slice(0, space);
  const target = space < 0 ? "" : trimmed.slice(space + 1).trim();
  const name = rawName.replace(/:$/, "");
  return { name, target, family: FAMILIES[name.toLowerCase()] ?? "other" };
}

/** Whether a tool target is a worktree-relative file path the UI can open. */
export function openablePath(tool: ToolCall): string | null {
  if (tool.family !== "read" && tool.family !== "edit") return null;
  const path = tool.target;
  if (!path || path === "files" || path.startsWith("/") || /\s/.test(path) || path.includes("..")) return null;
  return path;
}

/**
 * Build the timeline. Each run's log starts with the opening-prompt marker and
 * the prompt's lines; continuations then write a "run N (Agent)" header. So a
 * prompt ended by that header belongs to run N, and one ended by the agent's
 * first event (or the end of the log) belongs to run 1, which has no header.
 */
export function buildTimeline(items: ActivityDto[]): TimelineEntry[] {
  const out: TimelineEntry[] = [];
  let prompt: string[] | null = null;

  const endPrompt = (run: number, agent: string | null) => {
    out.push({ kind: "run", run, agent });
    const text = (prompt ?? []).join("\n").trim();
    if (text) out.push({ kind: "prompt", text });
    prompt = null;
  };

  for (const item of items) {
    const line = item.text.trim();
    if (item.kind === "output" && OPENING_PROMPT.test(line)) {
      if (prompt !== null) endPrompt(1, null);
      prompt = [];
      continue;
    }
    const header = item.kind === "output" ? RUN_HEADER.exec(line) : null;
    if (header) {
      if (prompt !== null) endPrompt(Number(header[1]), header[2]);
      else out.push({ kind: "run", run: Number(header[1]), agent: header[2] });
      continue;
    }
    if (prompt !== null) {
      if (item.kind === "output" || item.kind === "error") {
        prompt.push(item.text);
        continue;
      }
      endPrompt(1, null);
    }
    if (item.kind === "tool") {
      out.push({ kind: "tool", tool: parseTool(item.text) });
    } else if (item.kind === "output") {
      const last = out[out.length - 1];
      if (last?.kind === "output") last.lines.push(item.text);
      else out.push({ kind: "output", lines: [item.text] });
    } else {
      out.push({ kind: item.kind, text: item.text });
    }
  }
  if (prompt !== null) endPrompt(1, null);
  return out;
}
