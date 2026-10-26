import { useState } from "react";

import { agentLabel, formatTime } from "../format";
import { listSessionMessagesPage, type MessageDto, type MessagePartDto } from "../ipc";
import Diff from "./Diff";
import { Highlight } from "./Highlight";
import { describeTool } from "./tool";

const PAGE = 200;

function clockTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

const EVENT_NOTE: Record<string, string> = {
  summary: "Summary",
  compaction: "Context compacted",
  title: "Title changed",
  mode: "Mode changed",
  pr_link: "Pull request linked",
  attachment: "Attachment",
};

function roleName(role: string, agentId: string): string {
  switch (role) {
    case "user":
      return "You";
    case "assistant":
      return agentLabel(agentId);
    case "tool":
      return "Tool";
    case "system":
      return "System";
    default:
      return "Note";
  }
}

/** Plain text with fenced code blocks pulled out into monospace blocks. */
function Prose({ text, terms }: { text: string; terms: string[] }) {
  const chunks = text.split(/```[^\n]*\n([\s\S]*?)```/g);
  return (
    <>
      {chunks.map((chunk, index) =>
        index % 2 === 1 ? (
          <pre key={index} className="code">
            <Highlight text={chunk.replace(/\n$/, "")} terms={terms} />
          </pre>
        ) : chunk.trim() === "" ? null : (
          <p key={index} className="prose">
            <Highlight text={chunk.replace(/^\n+|\n+$/g, "")} terms={terms} />
          </p>
        ),
      )}
    </>
  );
}

function ToolPart({ part, open, terms }: { part: MessagePartDto; open: boolean; terms: string[] }) {
  const view = describeTool(part.content_json, part.text);
  const isResult = part.kind === "tool_result";
  return (
    <details className={`tool${view.isError ? " tool--error" : ""}`} open={open}>
      <summary className="tool__summary">
        <span className="tool__name">{isResult ? "Output" : view.name}</span>
        {view.detail && <code className="tool__detail">{view.detail}</code>}
        {view.isError && <span className="flag flag--danger">failed</span>}
      </summary>
      {view.patch ? (
        <Diff text={view.patch} label={`Patch from ${view.name}`} />
      ) : (
        <pre className="code tool__body">
          <Highlight text={view.body} terms={terms} />
        </pre>
      )}
    </details>
  );
}

function Part({ part, toolsOpen, terms }: { part: MessagePartDto; toolsOpen: boolean; terms: string[] }) {
  switch (part.kind) {
    case "opaque":
      return <p className="part-note">Encrypted block. Lore keeps it but cannot show it.</p>;
    case "thinking":
      return part.text ? (
        <details className="thinking">
          <summary>Thinking</summary>
          <p className="prose">{part.text}</p>
        </details>
      ) : null;
    case "tool_use":
    case "tool_result":
      return <ToolPart part={part} open={toolsOpen} terms={terms} />;
    default:
      if (part.text != null) return <Prose text={part.text} terms={terms} />;
      if (part.content_json != null) return <pre className="code">{part.content_json}</pre>;
      return null;
  }
}

function Turn({
  message,
  agentId,
  toolsOpen,
  terms,
}: {
  message: MessageDto;
  agentId: string;
  toolsOpen: boolean;
  terms: string[];
}) {
  const note = EVENT_NOTE[message.event_kind];
  // Agents file tool output under the user role; label it by what it is.
  const outputOnly = message.parts.length > 0 && message.parts.every((p) => p.kind === "tool_result");
  const toolOnly = message.parts.length > 0 && message.parts.every((p) => p.kind === "tool_use" || p.kind === "tool_result");
  const role = outputOnly ? "tool" : message.role;
  return (
    <article
      className={`turn turn--${role}${toolOnly ? " turn--compact" : ""}${message.is_sidechain ? " turn--side" : ""}`}
      aria-label={`${roleName(role, agentId)}, message ${message.seq}`}
    >
      <header className="turn__who">
        <span className="turn__role">{roleName(role, agentId)}</span>
        {message.is_sidechain && <span className="turn__side">subagent</span>}
        {message.ts != null && (
          <time className="turn__time" title={formatTime(message.ts)}>
            {clockTime(message.ts)}
          </time>
        )}
      </header>
      <div className="turn__body">
        {note && <p className="turn__note">{note}</p>}
        {message.parts.map((part) => (
          <Part key={part.ordinal} part={part} toolsOpen={toolsOpen} terms={terms} />
        ))}
      </div>
    </article>
  );
}

/**
 * The conversation, oldest first, paged from the backend 200 messages at a
 * time. Tool calls start collapsed so the human turns read as a dialogue.
 */
export default function Transcript({
  sessionId,
  agentId,
  initial,
  initialCursor,
  total,
  terms,
}: {
  sessionId: string;
  agentId: string;
  initial: MessageDto[];
  initialCursor: string | null;
  total: number;
  terms: string[];
}) {
  const [messages, setMessages] = useState(initial);
  const [cursor, setCursor] = useState(initialCursor);
  const [more, setMore] = useState<"idle" | "loading" | "failed">("idle");
  const [toolsOpen, setToolsOpen] = useState(false);

  async function loadMore() {
    if (cursor === null || more === "loading") return;
    setMore("loading");
    try {
      const page = await listSessionMessagesPage(sessionId, PAGE, cursor);
      setMessages((prev) => [...prev, ...page.messages]);
      setCursor(page.next_cursor);
      setMore("idle");
    } catch {
      setMore("failed");
    }
  }

  if (messages.length === 0) {
    return <p className="quiet">This session has no readable messages.</p>;
  }

  return (
    <div className="transcript">
      <div className="transcript__bar">
        <span className="quiet">
          {messages.length < total ? `${messages.length} of ${total} messages` : `${messages.length} messages`}
        </span>
        <button type="button" className="btn btn--quiet" aria-pressed={toolsOpen} onClick={() => setToolsOpen((v) => !v)}>
          {toolsOpen ? "Collapse tool calls" : "Expand tool calls"}
        </button>
      </div>
      {messages.map((message) => (
        <Turn
          key={`${message.id}-${toolsOpen ? "open" : "closed"}`}
          message={message}
          agentId={agentId}
          toolsOpen={toolsOpen}
          terms={terms}
        />
      ))}
      {cursor !== null && (
        <div className="transcript__more">
          <button type="button" className="btn" disabled={more === "loading"} onClick={() => void loadMore()}>
            {more === "loading" ? "Loading messages…" : "Load more messages"}
          </button>
          {more === "failed" && (
            <span className="quiet" role="alert">
              Could not load more messages. Try again.
            </span>
          )}
        </div>
      )}
    </div>
  );
}
