import { looksLikePatch } from "./Diff";

/** A tool call or result, reduced to what the transcript shows by default. */
export interface ToolView {
  name: string;
  /** One line that says what the call did: a command, a path, a pattern. */
  detail: string | null;
  /** Patch text to render as a diff, when the call carried one. */
  patch: string | null;
  /** The full payload, pretty-printed. */
  body: string;
  isError: boolean;
}

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

function parse(text: string): Json | undefined {
  try {
    return JSON.parse(text) as Json;
  } catch {
    return undefined;
  }
}

function isObject(value: Json | undefined): value is { [key: string]: Json } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function str(value: Json | undefined): string | null {
  if (typeof value === "string") return value;
  if (Array.isArray(value) && value.every((v) => typeof v === "string")) return value.join(" ");
  return null;
}

const DETAIL_KEYS = ["command", "cmd", "file_path", "path", "pattern", "url", "query", "description"];

function firstLine(text: string): string {
  const line = text.split("\n").find((l) => l.trim() !== "") ?? "";
  return line.length > 160 ? `${line.slice(0, 159)}…` : line;
}

function editAsPatch(input: { [key: string]: Json }): string | null {
  const oldText = str(input.old_string);
  const newText = str(input.new_string);
  if (oldText === null || newText === null) return null;
  const path = str(input.file_path) ?? "file";
  const minus = oldText.split("\n").map((l) => `-${l}`);
  const plus = newText.split("\n").map((l) => `+${l}`);
  return [`--- ${path}`, `+++ ${path}`, ...minus, ...plus].join("\n");
}

/** Read a tool part's JSON payload from either agent's shape. */
export function describeTool(contentJson: string | null, text: string | null): ToolView {
  const raw = contentJson === null ? undefined : parse(contentJson);
  const block = isObject(raw) ? raw : {};
  const name = str(block.name) ?? str(block.type) ?? "tool";
  const argText = str(block.arguments);
  const input: Json | undefined = block.input ?? (argText !== null ? (parse(argText) ?? argText) : undefined);

  let detail: string | null = null;
  let patch: string | null = null;
  if (typeof input === "string") {
    detail = firstLine(input);
    if (looksLikePatch(input)) patch = input;
  } else if (isObject(input)) {
    for (const key of DETAIL_KEYS) {
      const value = str(input[key]);
      if (value) {
        detail = firstLine(value);
        break;
      }
    }
    const embedded = str(input.patch) ?? str(input.input);
    patch = embedded && looksLikePatch(embedded) ? embedded : editAsPatch(input);
  }
  if (detail === null && text) detail = firstLine(text);

  const payload = input ?? raw;
  const body =
    text ??
    (typeof payload === "string"
      ? payload
      : payload === undefined
        ? (contentJson ?? "")
        : JSON.stringify(payload, null, 2));
  return { name, detail, patch, body, isError: block.is_error === true };
}
