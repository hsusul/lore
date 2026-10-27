import { clsx } from "clsx";
import {
  Bot,
  Brain,
  ChevronRight,
  FilePenLine,
  FilePlus,
  FileText,
  Globe,
  Layers,
  ListTodo,
  SquareTerminal,
  TextSearch,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { agentLabel, formatClock, formatTime } from "../format";
import type { FileEventDto } from "../ipc";
import type { ToolCallMode } from "../hooks/usePreferences";
import { CodeFrame, CopyButton, Counts, PathText } from "./code";
import Diff from "./Diff";
import { StoredPatch } from "./FileChanges";
import { Highlight } from "./Highlight";
import { summarizeRun, verbOf, type ToolCall, type ToolKind, type ToolResult } from "./tool";
import type { Block, Item } from "./transcript-model";
import { Button, ICON } from "./ui/button";

const ICONS: Record<ToolKind, LucideIcon> = {
  read: FileText,
  edit: FilePenLine,
  write: FilePlus,
  run: SquareTerminal,
  search: TextSearch,
  fetch: Globe,
  task: Bot,
  plan: ListTodo,
  other: Wrench,
};

/** Lines an output block shows before it scrolls (240px of 18px lines, less padding). */
const OUTPUT_LINES = 12;

const ROLE_LABEL: Record<string, string> = { system: "System" };

export const callAnchor = (key: string) => `call-${key}`;

function exitLabel(result: ToolResult): string {
  return result.exitCode !== null && result.exitCode !== 0 ? `Exit ${result.exitCode}` : "Failed";
}

const ROW =
  "group -ml-1.5 flex h-tool-row max-w-full flex-none items-center gap-1.5 self-start rounded-control px-1.5 text-label-2 outline-none focus-visible:shadow-focus";

function Chevron() {
  return (
    <ChevronRight
      className={clsx(
        ICON.small,
        "invisible transition-transform duration-disclosure ease-out group-hover:visible group-focus-visible:visible group-aria-expanded:visible group-aria-expanded:rotate-90",
      )}
      aria-hidden="true"
    />
  );
}

/** Plain text with fenced code pulled out into code blocks. */
function Prose({ text, terms }: { text: string; terms: string[] }) {
  const chunks = text.split(/```[^\n]*\n([\s\S]*?)```/g);
  return (
    <>
      {chunks.map((chunk, index) =>
        index % 2 === 1 ? (
          <CodeFrame key={index}>
            <pre className="px-3">
              <Highlight text={chunk.replace(/\n$/, "")} terms={terms} />
            </pre>
          </CodeFrame>
        ) : chunk.trim() === "" ? null : (
          <p key={index} className="max-w-measure whitespace-pre-wrap break-words">
            <Highlight text={chunk.replace(/^\n+|\n+$/g, "")} terms={terms} />
          </p>
        ),
      )}
    </>
  );
}

/** A tool result as a bounded output block (spec 9.4). Only output the session recorded. */
function Output({ call, result, terms }: { call: ToolCall; result: ToolResult; terms: string[] }) {
  const [all, setAll] = useState(false);
  if (result.text.trim() === "") {
    return <p className="text-callout text-label-2">{result.failed ? `${exitLabel(result)}, with no output.` : "No output."}</p>;
  }
  const lines = result.text.replace(/\n$/, "").split("\n").length;
  const command = call.kind === "run" ? (call.argument?.text ?? null) : null;
  return (
    <>
      <CodeFrame
        label={`Output of ${verbOf(call)}${call.argument ? ` ${call.argument.text}` : ""}`}
        failed={result.failed}
        bounded={!all}
        header={
          <>
            {command ? (
              <span className="min-w-0 flex-1 truncate font-mono text-label">{command}</span>
            ) : (
              <span className="flex-1 text-label-2">Output</span>
            )}
            {result.failed && <span className="flex-none text-red-text">{exitLabel(result)}</span>}
            <CopyButton text={result.text} label="Copy Output" />
          </>
        }
      >
        <pre className="px-3">
          <Highlight text={result.text} terms={terms} />
        </pre>
      </CodeFrame>
      {lines > OUTPUT_LINES && (
        <button type="button" className={clsx(ROW, "text-callout")} onClick={() => setAll((v) => !v)}>
          {all ? "Show Less" : `Show All ${lines.toLocaleString()} Lines`}
        </button>
      )}
    </>
  );
}

function RawInput({ json }: { json: string }) {
  const [shown, setShown] = useState(false);
  return (
    <>
      <button type="button" aria-expanded={shown} className={clsx(ROW, "text-callout")} onClick={() => setShown((v) => !v)}>
        <ChevronRight className={clsx(ICON.small, "transition-transform duration-disclosure ease-out group-aria-expanded:rotate-90")} aria-hidden="true" />
        {shown ? "Hide Raw Input" : "Show Raw Input"}
      </button>
      {shown && (
        <CodeFrame label="Raw input">
          <pre className="px-3">{json}</pre>
        </CodeFrame>
      )}
    </>
  );
}

/**
 * What an expanded call shows (spec 9.4): its short inputs, its patch or written content,
 * the output it recorded, and the raw JSON behind a disclosure. The file's recorded patch
 * stands in for the call's own only when this call alone changed that file.
 */
function ToolDetail({ call, file, terms }: { call: ToolCall; file: FileEventDto | null; terms: string[] }) {
  const argument = call.argument ? ` ${call.argument.text}` : "";
  return (
    <div className="flex min-w-0 flex-col gap-1.5 pb-1">
      {call.fields.length > 0 && (
        <dl className="grid max-w-measure grid-cols-facts gap-x-3 gap-y-0.5 text-callout">
          {call.fields.map(([label, value]) => (
            <div key={label} className="contents">
              <dt className="text-label-2">{label}</dt>
              <dd className="min-w-0 break-words">
                <Highlight text={value} terms={terms} />
              </dd>
            </div>
          ))}
        </dl>
      )}
      {file?.has_patch ? (
        <StoredPatch file={file} />
      ) : (
        call.patch && <Diff text={call.patch} label={`Patch from ${verbOf(call)}${argument}`} path={call.paths[0] ?? null} />
      )}
      {call.content !== null && (
        <CodeFrame
          label={`Content written to${argument}`}
          header={
            <>
              {call.paths[0] ? <PathText path={call.paths[0]} className="flex-1" /> : <span className="flex-1" />}
              <CopyButton text={call.content} label="Copy Content" />
            </>
          }
        >
          <pre className="px-3">
            <Highlight text={call.content} terms={terms} />
          </pre>
        </CodeFrame>
      )}
      {call.result && <Output call={call} result={call.result} terms={terms} />}
      {call.raw && <RawInput json={call.raw} />}
    </div>
  );
}

function ToolRow({
  call,
  open,
  onToggle,
  file,
  terms,
}: {
  call: ToolCall;
  open: boolean;
  onToggle: () => void;
  file: FileEventDto | null;
  terms: string[];
}) {
  const Icon = ICONS[call.kind];
  const failed = call.result?.failed ? call.result : null;
  return (
    <>
      <button id={callAnchor(call.key)} type="button" aria-expanded={open} className={ROW} onClick={onToggle}>
        <Icon className={ICON.body} aria-hidden="true" />
        <span className="flex-none text-callout">{verbOf(call)}</span>
        {/* Spaces keep the row's accessible name readable; flex layout drops them visually. */}
        {call.argument && " "}
        {call.argument &&
          (call.argument.isPath ? (
            <PathText path={call.argument.text} terms={terms} />
          ) : (
            <span className="min-w-0 truncate font-mono text-callout text-label">
              <Highlight text={call.argument.text} terms={terms} />
            </span>
          ))}
        {/* An open call shows its counts and exit status in its own blocks. */}
        {!open && file && " "}
        {!open && file && <Counts added={file.lines_added} removed={file.lines_removed} quiet />}
        {!open && failed && " "}
        {!open && failed && <span className="flex-none text-callout text-red-text">{exitLabel(failed)}</span>}
        <Chevron />
      </button>
      {open && <ToolDetail call={call} file={file} terms={terms} />}
    </>
  );
}

interface Disclosures {
  isOpen: (key: string, fallback: boolean) => boolean;
  toggle: (key: string, fallback: boolean) => void;
}

function Tools({
  item,
  mode,
  disclosures,
  fileFor,
  terms,
}: {
  item: Extract<Item, { kind: "tools" }>;
  mode: ToolCallMode;
  disclosures: Disclosures;
  fileFor: (call: ToolCall) => FileEventDto | null;
  terms: string[];
}) {
  const expanded = mode === "expanded";
  const rows = item.calls.map((call) => (
    <ToolRow
      key={call.key}
      call={call}
      open={disclosures.isOpen(call.key, expanded)}
      onToggle={() => disclosures.toggle(call.key, expanded)}
      file={fileFor(call)}
      terms={terms}
    />
  ));
  if (item.calls.length === 1) return <>{rows}</>;
  // A run of calls collapses into one row that says what it did (spec 9.4).
  const runKey = `run:${item.key}`;
  const open = disclosures.isOpen(runKey, expanded);
  const failures = item.calls.filter((c) => c.result?.failed).length;
  return (
    <>
      <button type="button" aria-expanded={open} className={ROW} onClick={() => disclosures.toggle(runKey, expanded)}>
        <Layers className={ICON.body} aria-hidden="true" />
        <span className="min-w-0 truncate text-callout">{summarizeRun(item.calls)}</span>
        {!open && failures > 0 && <span className="flex-none text-callout text-red-text">{failures} failed</span>}
        <Chevron />
      </button>
      {open && <div className="flex min-w-0 flex-col gap-1.5 pl-5">{rows}</div>}
    </>
  );
}

function Thinking({ text, open, onToggle, terms }: { text: string; open: boolean; onToggle: () => void; terms: string[] }) {
  return (
    <>
      <button type="button" aria-expanded={open} className={ROW} onClick={onToggle}>
        <Brain className={ICON.body} aria-hidden="true" />
        <span className="text-callout">Thinking</span>
        <Chevron />
      </button>
      {open && (
        <p className="max-w-measure whitespace-pre-wrap break-words text-label-2">
          <Highlight text={text} terms={terms} />
        </p>
      )}
    </>
  );
}

function Items({
  items,
  mode,
  disclosures,
  fileFor,
  terms,
}: {
  items: Item[];
  mode: ToolCallMode;
  disclosures: Disclosures;
  fileFor: (call: ToolCall) => FileEventDto | null;
  terms: string[];
}) {
  return (
    <>
      {items.map((item): ReactNode => {
        switch (item.kind) {
          case "text":
            return <Prose key={item.key} text={item.text} terms={terms} />;
          case "thinking":
            return (
              <Thinking
                key={item.key}
                text={item.text}
                open={disclosures.isOpen(item.key, false)}
                onToggle={() => disclosures.toggle(item.key, false)}
                terms={terms}
              />
            );
          case "note":
            return (
              <p key={item.key} className="text-callout text-label-2">
                {item.text}
              </p>
            );
          case "data":
            return (
              <CodeFrame key={item.key}>
                <pre className="px-3">{item.json}</pre>
              </CodeFrame>
            );
          case "tools":
            return <Tools key={item.key} item={item} mode={mode} disclosures={disclosures} fileFor={fileFor} terms={terms} />;
        }
      })}
    </>
  );
}

/** The run of calls holding `key`, so a call revealed from the Files tab starts visible. */
function runHolding(blocks: Block[], key: string): string | null {
  for (const block of blocks) {
    for (const item of block.items) {
      if (item.kind === "tools" && item.calls.length > 1 && item.calls.some((c) => c.key === key)) return `run:${item.key}`;
    }
  }
  return null;
}

export interface MoreMessages {
  status: "idle" | "loading" | "failed";
  onMore: () => void;
}

/**
 * The conversation (spec 9.4), oldest first: user turns as filled blocks, assistant runs
 * as plain prose under the agent's name, tool calls as quiet one-line rows. "Show tool
 * calls" decides whether calls start collapsed or open; the user can flip any one.
 */
export default function Transcript({
  blocks,
  agentId,
  mode,
  terms,
  fileFor,
  more,
  reveal,
  onRevealed,
}: {
  blocks: Block[];
  agentId: string;
  mode: ToolCallMode;
  terms: string[];
  fileFor: (call: ToolCall) => FileEventDto | null;
  more: MoreMessages | null;
  reveal: string | null;
  onRevealed: () => void;
}) {
  const [open, setOpen] = useState<Record<string, boolean>>(() => {
    const run = reveal === null ? null : runHolding(blocks, reveal);
    return run === null ? {} : { [run]: true };
  });
  const disclosures: Disclosures = {
    isOpen: (key, fallback) => open[key] ?? fallback,
    toggle: (key, fallback) => setOpen((prev) => ({ ...prev, [key]: !(prev[key] ?? fallback) })),
  };

  useEffect(() => {
    if (reveal === null) return;
    const row = document.getElementById(callAnchor(reveal));
    row?.scrollIntoView({ block: "center" });
    row?.focus();
    onRevealed();
  }, [reveal, onRevealed]);

  if (blocks.length === 0) {
    return <p className="text-callout text-label-2">This session has no readable messages.</p>;
  }

  const agent = agentLabel(agentId);
  const shared = { mode, disclosures, fileFor, terms };
  return (
    <div className="flex flex-col gap-5">
      {blocks.map((block) => {
        switch (block.kind) {
          case "user":
            return (
              <div
                key={block.key}
                role="group"
                aria-label="You"
                title={block.ts !== null ? formatTime(block.ts) : undefined}
                className="flex min-w-0 max-w-user-turn flex-col gap-1.5 self-start rounded-group bg-fill-2 px-3 py-2.5"
              >
                <Items items={block.items} {...shared} />
              </div>
            );
          case "agent":
            return (
              <div key={block.key} role="group" aria-label={agent} className="flex min-w-0 flex-col gap-1.5">
                <p className="flex items-baseline gap-1.5 text-subheadline font-semibold text-label-2">
                  {block.sidechain ? `${agent} subagent` : agent}
                  {block.ts !== null && (
                    <time className="font-regular tabular-nums" dateTime={new Date(block.ts).toISOString()} title={formatTime(block.ts)}>
                      {formatClock(block.ts)}
                    </time>
                  )}
                </p>
                <Items items={block.items} {...shared} />
              </div>
            );
          case "aside":
            return (
              <div key={block.key} className="flex min-w-0 max-w-measure flex-col gap-1 text-callout text-label-2">
                <p className="font-semibold">{ROLE_LABEL[block.role] ?? "Note"}</p>
                <Items items={block.items} {...shared} />
              </div>
            );
        }
      })}
      {more && (
        <div className="flex items-center gap-3">
          <Button disabled={more.status === "loading"} onClick={more.onMore}>
            {more.status === "loading" ? "Loading Messages…" : "Load More Messages"}
          </Button>
          {more.status === "failed" && (
            <span className="text-callout text-label-2" role="alert">
              Could not load more messages. Try again.
            </span>
          )}
        </div>
      )}
    </div>
  );
}
