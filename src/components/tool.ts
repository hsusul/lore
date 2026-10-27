import { plural } from "../format";
import type { MessagePartDto } from "../ipc";
import { looksLikePatch, patchPaths } from "./Diff";

/** What a tool call did, which decides its verb, icon, and how a run of calls is summed up. */
export type ToolKind = "read" | "edit" | "write" | "run" | "search" | "fetch" | "task" | "plan" | "other";

const KIND_BY_NAME: Record<string, ToolKind> = {
  read: "read",
  read_file: "read",
  view: "read",
  notebookread: "read",
  edit: "edit",
  multiedit: "edit",
  notebookedit: "edit",
  apply_patch: "edit",
  str_replace_editor: "edit",
  str_replace_based_edit_tool: "edit",
  write: "write",
  write_file: "write",
  bash: "run",
  shell: "run",
  local_shell: "run",
  local_shell_call: "run",
  exec_command: "run",
  unified_exec: "run",
  grep: "search",
  glob: "search",
  ls: "search",
  rg: "search",
  search: "search",
  webfetch: "fetch",
  websearch: "fetch",
  web_search: "fetch",
  task: "task",
  agent: "task",
  todowrite: "plan",
  update_plan: "plan",
};

/** The verb on a call's row ("Read", "Ran") and the clause it adds to a run's summary row. */
export const KIND_WORDS: Record<ToolKind, { verb: string; tally: (n: number, name: string) => string }> = {
  read: { verb: "Read", tally: (n) => `read ${plural(n, "file")}` },
  edit: { verb: "Edited", tally: (n) => `edited ${plural(n, "file")}` },
  write: { verb: "Wrote", tally: (n) => `wrote ${plural(n, "file")}` },
  run: { verb: "Ran", tally: (n) => `ran ${plural(n, "command")}` },
  search: { verb: "Searched", tally: (n) => `searched ${n === 1 ? "once" : `${n} times`}` },
  fetch: { verb: "Fetched", tally: (n) => `fetched ${plural(n, "page")}` },
  task: { verb: "Delegated", tally: (n) => `delegated ${plural(n, "task")}` },
  plan: { verb: "Updated plan", tally: (n) => (n === 1 ? "updated the plan" : `updated the plan ${n} times`) },
  other: { verb: "", tally: (n, name) => (n === 1 ? `used ${name}` : `used ${name} ${n} times`) },
};

export interface ToolResult {
  text: string;
  failed: boolean;
  /** Present only when the result carries one. */
  exitCode: number | null;
}

/** A tool call as the transcript shows it, with its result when one was paired to it. */
export interface ToolCall {
  /** `${messageId}:${ordinal}`, unique within a session. */
  key: string;
  ts: number | null;
  name: string;
  kind: ToolKind;
  /** The one input that says what the call did: a path, a command, a pattern. */
  argument: { text: string; isPath: boolean } | null;
  /** The other short inputs worth reading, as label and value. */
  fields: [string, string][];
  /** A patch carried in the input, as a unified diff. */
  patch: string | null;
  /** File content written by the call. */
  content: string | null;
  /** Pretty-printed input JSON, for "Show Raw Input". */
  raw: string | null;
  /** Files the call changed, for matching the session's file events. */
  paths: string[];
  /** The native call id, when the agent records one. */
  id: string | null;
  result: ToolResult | null;
}

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type JsonObject = { [key: string]: Json };

function parse(text: string | null): Json | undefined {
  if (text === null) return undefined;
  try {
    return JSON.parse(text) as Json;
  } catch {
    return undefined;
  }
}

function isObject(value: Json | undefined): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function str(value: Json | undefined): string | null {
  return typeof value === "string" ? value : null;
}

function firstLine(text: string): string {
  const line = text.split("\n").find((l) => l.trim() !== "") ?? "";
  return line.length > 200 ? `${line.slice(0, 199)}…` : line;
}

/** A command as one line; `["bash", "-lc", "npm test"]` reads as `npm test`. */
function commandText(value: Json | undefined): string | null {
  if (typeof value === "string") return value;
  if (!Array.isArray(value) || !value.every((v) => typeof v === "string")) return null;
  const words = value as string[];
  if (words.length === 3 && (words[1] === "-lc" || words[1] === "-c")) return words[2];
  return words.join(" ");
}

const PATH_KEYS = ["file_path", "path", "notebook_path"];
const ARGUMENT_KEYS = ["command", "cmd", ...PATH_KEYS, "pattern", "url", "query", "description"];
/** Inputs shown in their own block (a patch, file content), too large for a field, or bookkeeping. */
const NOT_FIELDS = new Set(["old_string", "new_string", "content", "patch", "input", "edits", "todos", "prompt", "type"]);

function label(key: string): string {
  const words = key.replace(/_/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

function editAsPatch(input: JsonObject): string | null {
  const edits = Array.isArray(input.edits) ? input.edits.filter(isObject) : [input];
  const path = str(input.file_path) ?? str(input.path) ?? "file";
  const lines: string[] = [];
  for (const edit of edits) {
    const oldText = str(edit.old_string);
    const newText = str(edit.new_string);
    if (oldText === null || newText === null) continue;
    lines.push(...oldText.split("\n").map((l) => `-${l}`), ...newText.split("\n").map((l) => `+${l}`));
  }
  return lines.length === 0 ? null : [`--- ${path}`, `+++ ${path}`, ...lines].join("\n");
}

/** Read a tool_use part from either agent's shape. */
export function readCall(part: MessagePartDto, messageId: string, ts: number | null): ToolCall {
  const raw = parse(part.content_json);
  const block = isObject(raw) ? raw : {};
  const name = str(block.name) ?? str(block.type) ?? "tool";
  const argText = str(block.arguments);
  const action = isObject(block.action) ? block.action : null;
  const input: Json | undefined = block.input ?? (argText !== null ? (parse(argText) ?? argText) : (action ?? undefined));
  const kind = KIND_BY_NAME[name.toLowerCase()] ?? "other";

  let argument: ToolCall["argument"] = null;
  let patch: string | null = null;
  let content: string | null = null;
  const fields: [string, string][] = [];
  const paths: string[] = [];

  if (typeof input === "string") {
    if (looksLikePatch(input)) {
      patch = input;
      paths.push(...patchPaths(input));
      argument = paths.length > 0 ? { text: paths[0], isPath: true } : null;
    } else {
      argument = { text: firstLine(input), isPath: false };
    }
  } else if (isObject(input)) {
    const key = ARGUMENT_KEYS.find((k) => (k === "command" || k === "cmd" ? commandText(input[k]) : str(input[k])));
    if (key) {
      const text = key === "command" || key === "cmd" ? commandText(input[key])! : str(input[key])!;
      argument = { text: firstLine(text), isPath: PATH_KEYS.includes(key) };
    }
    for (const [k, value] of Object.entries(input)) {
      if (k === key || NOT_FIELDS.has(k)) continue;
      if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
        const text = firstLine(String(value));
        if (text !== "") fields.push([label(k), text]);
      }
    }
    const embedded = str(input.patch) ?? str(input.input);
    patch = embedded !== null && looksLikePatch(embedded) ? embedded : editAsPatch(input);
    if (kind === "write") content = str(input.content);
    if (kind === "edit" || kind === "write") {
      const path = PATH_KEYS.map((k) => str(input[k])).find((p) => p !== null);
      if (path) paths.push(path);
      else if (patch) paths.push(...patchPaths(patch));
    }
  }

  return {
    key: `${messageId}:${part.ordinal}`,
    ts,
    name,
    kind,
    argument,
    fields,
    patch,
    content,
    raw: isObject(input) ? JSON.stringify(input, null, 2) : input === undefined && raw !== undefined ? JSON.stringify(raw, null, 2) : null,
    paths,
    id: str(block.id) ?? str(block.call_id),
    result: null,
  };
}

/**
 * Read a tool_result part. Claude stores only the result's content; Codex stores the
 * whole output item, whose `status`, `is_error`, `exit_code` and `error` say whether it
 * failed (the same rules the Codex adapter applies).
 */
export function readResult(part: MessagePartDto): { id: string | null; result: ToolResult } {
  const payload = parse(part.content_json);
  const item = isObject(payload) ? payload : {};
  let text = part.text ?? str(payload) ?? "";
  let exitCode = typeof item.exit_code === "number" ? item.exit_code : null;
  // Codex exec output wraps the text: {"output": "...", "metadata": {"exit_code": 1}}.
  const inner = parse(text);
  if (isObject(inner) && typeof inner.output === "string") {
    text = inner.output;
    const meta = isObject(inner.metadata) ? inner.metadata : {};
    if (exitCode === null && typeof meta.exit_code === "number") exitCode = meta.exit_code;
  }
  const status = str(item.status);
  const failed =
    (status !== null && !["completed", "success", "ok"].includes(status)) ||
    item.is_error === true ||
    (exitCode !== null && exitCode !== 0) ||
    (item.error !== undefined && item.error !== null && item.error !== false);
  return { id: str(item.call_id) ?? str(item.tool_use_id), result: { text, failed, exitCode } };
}

/** "Read 2 files, ran 1 command": one clause per kind of call, in order of first use. */
export function summarizeRun(calls: ToolCall[]): string {
  const groups = new Map<string, { kind: ToolKind; name: string; n: number }>();
  for (const call of calls) {
    const id = call.kind === "other" ? `other:${call.name}` : call.kind;
    const group = groups.get(id) ?? { kind: call.kind, name: call.name, n: 0 };
    group.n += 1;
    groups.set(id, group);
  }
  const text = [...groups.values()].map((g) => KIND_WORDS[g.kind].tally(g.n, g.name)).join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** The verb on a call's row: the kind's verb, or the tool's own name. */
export function verbOf(call: ToolCall): string {
  return KIND_WORDS[call.kind].verb || call.name;
}
