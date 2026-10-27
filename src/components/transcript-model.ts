import type { FileEventDto, MessageDto } from "../ipc";
import { readCall, readResult, type ToolCall } from "./tool";

/** One piece of a turn, in order. Consecutive tool calls share one `tools` item. */
export type Item =
  | { kind: "text"; key: string; text: string }
  | { kind: "thinking"; key: string; text: string }
  | { kind: "note"; key: string; text: string }
  | { kind: "data"; key: string; json: string }
  | { kind: "tools"; key: string; calls: ToolCall[] };

/**
 * The transcript as the reader lays it out (spec 9.4): each user turn is its own block,
 * consecutive assistant turns form one run under the agent's name, and system or meta
 * messages stand aside. Tool results never get a block; they attach to their call.
 */
export type Block =
  | { kind: "user"; key: string; ts: number | null; items: Item[] }
  | { kind: "agent"; key: string; ts: number | null; sidechain: boolean; items: Item[] }
  | { kind: "aside"; key: string; role: string; items: Item[] };

const EVENT_NOTE: Record<string, string> = {
  summary: "Summary",
  compaction: "Context compacted",
  title: "Title changed",
  mode: "Mode changed",
  pr_link: "Pull request linked",
  attachment: "Attachment",
};

export const OPAQUE_NOTE = "Encrypted block. Lore keeps it but cannot show it.";

function isAgentTurn(message: MessageDto): boolean {
  if (message.role === "assistant" || message.role === "tool") return true;
  // Claude files tool results under the user role; they belong to the agent's run.
  return message.parts.length > 0 && message.parts.every((p) => p.kind === "tool_result");
}

export function buildTranscript(messages: MessageDto[]): Block[] {
  const blocks: Block[] = [];
  const byId = new Map<string, ToolCall>();
  // Calls still waiting for a result, oldest first. Claude's stored results carry no
  // call id, so they pair in order, which is how Claude emits them.
  const waiting: ToolCall[] = [];

  for (const message of messages) {
    let block: Block;
    const last = blocks[blocks.length - 1];
    if (isAgentTurn(message)) {
      if (last?.kind === "agent" && last.sidechain === message.is_sidechain) block = last;
      else {
        block = { kind: "agent", key: message.id, ts: message.ts, sidechain: message.is_sidechain, items: [] };
        blocks.push(block);
      }
    } else {
      block =
        message.role === "user"
          ? { kind: "user", key: message.id, ts: message.ts, items: [] }
          : { kind: "aside", key: message.id, role: message.role, items: [] };
      blocks.push(block);
    }
    const items = block.items;
    const addCall = (call: ToolCall) => {
      const tail = items[items.length - 1];
      if (tail?.kind === "tools") tail.calls.push(call);
      else items.push({ kind: "tools", key: call.key, calls: [call] });
    };

    const note = EVENT_NOTE[message.event_kind];
    if (note) items.push({ kind: "note", key: `${message.id}:note`, text: note });

    for (const part of message.parts) {
      const key = `${message.id}:${part.ordinal}`;
      switch (part.kind) {
        case "tool_use": {
          const call = readCall(part, message.id, message.ts);
          if (call.id) byId.set(call.id, call);
          waiting.push(call);
          addCall(call);
          break;
        }
        case "tool_result": {
          const { id, result } = readResult(part);
          const call = (id !== null ? byId.get(id) : undefined) ?? waiting.find((c) => c.result === null);
          if (call && call.result === null) {
            call.result = result;
            const at = waiting.indexOf(call);
            if (at >= 0) waiting.splice(at, 1);
          } else {
            addCall({ ...readCall({ ...part, content_json: null }, message.id, message.ts), name: "Output", result });
          }
          break;
        }
        case "thinking":
          if (part.text) items.push({ kind: "thinking", key, text: part.text });
          break;
        case "opaque":
          items.push({ kind: "note", key, text: OPAQUE_NOTE });
          break;
        default:
          if (part.text != null && part.text.trim() !== "") items.push({ kind: "text", key, text: part.text });
          else if (part.text == null && part.content_json != null) items.push({ kind: "data", key, json: part.content_json });
      }
    }
  }
  return blocks;
}

export function callsOf(blocks: Block[]): ToolCall[] {
  return blocks.flatMap((b) => b.items.flatMap((i) => (i.kind === "tools" ? i.calls : [])));
}

function samePath(a: string, b: string): boolean {
  const x = a.replace(/^\.\//, "");
  const y = b.replace(/^\.\//, "");
  return x === y || x.endsWith(`/${y}`) || y.endsWith(`/${x}`);
}

/** The calls that changed each file event's path, oldest first. */
export function callsByFile(files: FileEventDto[], calls: ToolCall[]): Map<string, ToolCall[]> {
  return new Map(files.map((file) => [file.id, calls.filter((c) => c.paths.some((p) => samePath(p, file.path)))]));
}

/**
 * The file event behind each call, when the pairing is one to one: the only call that
 * changed that file. Only then can the file's recorded patch and counts stand for the call.
 */
export function fileByCall(files: FileEventDto[], byFile: Map<string, ToolCall[]>): Map<string, FileEventDto> {
  const out = new Map<string, FileEventDto>();
  for (const file of files) {
    const calls = byFile.get(file.id) ?? [];
    if (calls.length === 1 && calls[0].paths.length === 1) out.set(calls[0].key, file);
  }
  return out;
}
