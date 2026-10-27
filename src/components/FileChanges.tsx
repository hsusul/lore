import { clsx } from "clsx";
import { ChevronRight } from "lucide-react";
import { useEffect, useState } from "react";

import { formatClock, plural } from "../format";
import { getFilePatch, type FileEventDto } from "../ipc";
import { CopyButton, Counts, PathText } from "./code";
import Diff from "./Diff";
import type { ToolCall } from "./tool";
import { ICON } from "./ui/button";

type PatchState = { status: "loading" } | { status: "ready"; text: string | null } | { status: "failed"; error: string };

/** A file event's recorded patch, fetched while `enabled`. */
function useFilePatch(id: string, enabled: boolean): PatchState | null {
  const [state, setState] = useState<PatchState | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    setState({ status: "loading" });
    getFilePatch(id).then(
      (text) => live && setState({ status: "ready", text }),
      (e: unknown) => live && setState({ status: "failed", error: String(e) }),
    );
    return () => {
      live = false;
    };
  }, [id, enabled]);
  return enabled ? state : null;
}

function PatchBody({ file, state, header }: { file: FileEventDto; state: PatchState | null; header: boolean }) {
  if (state === null) return null;
  if (state.status === "loading") return <p className="text-callout text-label-2">Loading the patch…</p>;
  if (state.status === "failed") {
    return (
      <p className="text-callout text-label-2" role="alert">
        Could not load the patch: {state.error}
      </p>
    );
  }
  if (state.text === null) return <p className="text-callout text-label-2">The patch for this change was not stored.</p>;
  return <Diff text={state.text} label={`Patch for ${file.path}`} path={file.path} header={header} />;
}

/** A file's recorded patch with its own header, for an expanded tool call in the transcript. */
export function StoredPatch({ file }: { file: FileEventDto }) {
  return <PatchBody file={file} state={useFilePatch(file.id, true)} header />;
}

const SOURCE: Record<string, string> = {
  agent_patch: "Session patch, no tool call",
  agent_tool_input: "From a tool call’s input",
  lore_capture: "Captured by Lore from the working tree",
};

const CHANGE: Record<string, string> = {
  create: "New file",
  delete: "Deleted",
  read: "Read only",
};

/**
 * Where a file change came from, in words (spec 9.5, 18.9). A time appears only when a
 * tool call that changed this path is found in the loaded transcript.
 */
export function provenance(file: FileEventDto, calls: ToolCall[]): string {
  const last = calls[calls.length - 1];
  const at = last?.ts != null ? ` at ${formatClock(last.ts)}` : "";
  const names = new Set(calls.map((c) => c.name));
  const name = names.size === 1 ? last.name : "tool";
  const article = /^[aeiou]/i.test(name) ? "an" : "a";
  const source =
    calls.length === 1
      ? `From ${article} ${name} call${at}`
      : calls.length > 1
        ? `From ${calls.length} ${name} calls${at ? `, the last${at}` : ""}`
        : (SOURCE[file.source] ?? file.source);
  const change = file.change_kind === "move" ? (file.old_path ? `Moved from ${file.old_path}` : "Moved") : CHANGE[file.change_kind];
  return change ? `${change}. ${source}` : source;
}

function editorHref(path: string): string {
  return `vscode://file/${encodeURI(path.startsWith("/") ? path : `/${path}`)}`;
}

function FileRow({ file, calls, onShowCall }: { file: FileEventDto; calls: ToolCall[]; onShowCall: (key: string) => void }) {
  const [open, setOpen] = useState(false);
  const patch = useFilePatch(file.id, open);
  const last = calls[calls.length - 1];
  const toggle = () => setOpen((v) => !v);
  return (
    <li className="flex flex-col gap-1">
      {/* The row is the patch's header: it sticks while its patch scrolls by. */}
      <div className="sticky top-0 z-10 -mx-1.5 grid grid-cols-file-row items-center gap-x-1.5 gap-y-0.5 bg-bg-content px-1.5 py-1.5">
        {file.has_patch ? (
          <ChevronRight
            className={clsx(ICON.small, "text-label-2 transition-transform duration-disclosure ease-out", open && "rotate-90")}
            aria-hidden="true"
            onClick={toggle}
          />
        ) : (
          <span />
        )}
        {file.has_patch ? (
          <button
            type="button"
            aria-expanded={open}
            className="flex min-w-0 rounded-control text-left outline-none focus-visible:shadow-focus"
            onClick={toggle}
          >
            <PathText path={file.path} />
          </button>
        ) : (
          <PathText path={file.path} />
        )}
        <span className="flex items-center gap-1">
          <Counts added={file.lines_added} removed={file.lines_removed} />
          {patch?.status === "ready" && patch.text !== null && <CopyButton text={patch.text} label="Copy Patch" />}
        </span>
        <p className="col-span-2 col-start-2 flex flex-wrap gap-x-3 text-callout text-label-2">
          <span>{provenance(file, calls)}</span>
          {last && (
            <button type="button" className="rounded-control text-link outline-none focus-visible:shadow-focus" onClick={() => onShowCall(last.key)}>
              Show in Conversation
            </button>
          )}
          <a className="rounded-control text-link outline-none focus-visible:shadow-focus" href={editorHref(file.path)} target="_blank" rel="noreferrer">
            Open in Editor
          </a>
        </p>
      </div>
      <PatchBody file={file} state={patch} header={false} />
    </li>
  );
}

/** The Files tab (spec 9.5): one summary sentence, then a disclosure row per file. */
export default function FileChanges({
  files,
  callsByFile,
  onShowCall,
}: {
  files: FileEventDto[];
  callsByFile: Map<string, ToolCall[]>;
  onShowCall: (key: string) => void;
}) {
  if (files.length === 0) {
    return <p className="text-callout text-label-2">This session did not touch any files Lore could see.</p>;
  }
  const changed = files.filter((f) => f.change_kind !== "read");
  const total = (key: "lines_added" | "lines_removed") =>
    changed.some((f) => f[key] !== null) ? changed.reduce((n, f) => n + (f[key] ?? 0), 0) : null;
  const added = total("lines_added");
  const removed = total("lines_removed");
  const counted = added !== null || removed !== null;
  return (
    <div className="flex flex-col gap-3">
      <p className="flex items-baseline gap-1.5 text-body">
        {changed.length > 0 ? `Edited ${plural(changed.length, "file")}${counted ? "," : ""}` : `Read ${plural(files.length, "file")}`}
        {changed.length > 0 && <Counts added={added} removed={removed} />}
      </p>
      <ul className="flex flex-col gap-3">
        {files.map((file) => (
          <FileRow key={file.id} file={file} calls={callsByFile.get(file.id) ?? []} onShowCall={onShowCall} />
        ))}
      </ul>
    </div>
  );
}
