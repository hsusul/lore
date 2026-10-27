import { clsx } from "clsx";

import { CodeFrame, CopyButton, Counts, PathText } from "./code";

const MAX_LINES = 1_000;
const MINUS = "−";

type LineKind = "file" | "hunk" | "add" | "del" | "context";

interface DiffLine {
  kind: LineKind;
  text: string;
  oldNo: number | null;
  newNo: number | null;
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;
const FILE_HEADER = /^(diff --git |index |\+\+\+ |--- |\*\*\* (Begin|End|Update|Add|Delete) )/;
/** apply_patch's per-file line, which still separates files once the envelope is hidden. */
const ENVELOPE_FILE = /^\*\*\* (Update|Add|Delete) File: /;

/** True when text looks like a unified diff or an apply_patch envelope. */
export function looksLikePatch(text: string): boolean {
  return /^(@@ |diff --git |\*\*\* Begin Patch|--- \S)/m.test(text);
}

/** The files a patch names, from `*** Update File:` envelopes or `+++ b/` headers. */
export function patchPaths(patch: string): string[] {
  const found = [...patch.matchAll(/^(?:\*\*\* (?:Update|Add|Delete) File: (.+)|\+\+\+ (?:b\/)?(.+))$/gm)]
    .map((m) => (m[1] ?? m[2]).trim())
    .filter((p) => p !== "/dev/null");
  return [...new Set(found)];
}

/** Classify each line and carry old/new line numbers through unified hunks. */
export function parseDiff(text: string): DiffLine[] {
  let oldNo: number | null = null;
  let newNo: number | null = null;
  return text
    .replace(/\n$/, "")
    .split("\n")
    .map((line): DiffLine => {
      const hunk = HUNK.exec(line);
      if (hunk) {
        oldNo = Number(hunk[1]);
        newNo = Number(hunk[2]);
        return { kind: "hunk", text: line, oldNo: null, newNo: null };
      }
      if (line.startsWith("@@")) return { kind: "hunk", text: line, oldNo: null, newNo: null };
      if (line.startsWith("diff --git ") || line.startsWith("*** ")) {
        oldNo = null;
        newNo = null;
      }
      // Inside a numbered hunk, `--- x` is a removed line that began with `--`.
      if (oldNo === null && FILE_HEADER.test(line)) {
        return { kind: "file", text: line, oldNo: null, newNo: null };
      }
      if (line.startsWith("+")) {
        const row = { kind: "add" as const, text: line, oldNo: null, newNo };
        if (newNo !== null) newNo += 1;
        return row;
      }
      if (line.startsWith("-")) {
        const row = { kind: "del" as const, text: line, oldNo, newNo: null };
        if (oldNo !== null) oldNo += 1;
        return row;
      }
      const row = { kind: "context" as const, text: line, oldNo, newNo };
      if (oldNo !== null) oldNo += 1;
      if (newNo !== null) newNo += 1;
      return row;
    });
}

const SIGN: Partial<Record<LineKind, string>> = { add: "+", del: MINUS };
const ROW: Record<LineKind, string> = {
  file: "text-label-2",
  hunk: "text-label-2",
  // Tints paint over the block's opaque content color, so the signs keep their contrast (spec 9.5).
  add: "bg-diff-add-bg",
  del: "bg-diff-del-bg",
  context: "",
};

/**
 * A patch (spec 9.5): an optional sticky header with the path, counts and a copy button,
 * two line-number columns when the patch has hunks, a sign column, and tinted rows. The
 * patch's own file headers are hidden; the header names the file instead.
 */
export default function Diff({
  text,
  label,
  path = null,
  header = true,
}: {
  text: string;
  label: string;
  path?: string | null;
  header?: boolean;
}) {
  const all = parseDiff(text);
  const shown = all.filter((line) => line.kind !== "file" || ENVELOPE_FILE.test(line.text));
  const lines = shown.slice(0, MAX_LINES);
  const hidden = shown.length - lines.length;
  const numbered = all.some((line) => line.oldNo !== null || line.newNo !== null);
  const added = all.filter((line) => line.kind === "add").length;
  const removed = all.filter((line) => line.kind === "del").length;
  const title = path ?? patchPaths(text)[0] ?? null;
  return (
    <CodeFrame
      label={label}
      header={
        header && (
          <>
            {title ? <PathText path={title} className="flex-1" /> : <span className="flex-1" />}
            <Counts added={added} removed={removed} />
            <CopyButton text={text} label="Copy Patch" />
          </>
        )
      }
    >
      <table className="w-max min-w-full border-collapse">
        <tbody>
          {lines.map((line, index) => {
            // The sign column carries +/−, so drop the patch's own marker (a space on context lines).
            const body = line.kind === "add" || line.kind === "del" || (line.kind === "context" && line.text.startsWith(" ")) ? line.text.slice(1) : line.text;
            return (
              <tr key={index} className={clsx("whitespace-pre", ROW[line.kind])}>
                {numbered && (
                  <>
                    <td className="w-line-no select-none pr-0.5 text-right tabular-nums text-label-2" aria-hidden="true">
                      {line.oldNo ?? ""}
                    </td>
                    <td className="w-line-no select-none pr-0.5 text-right tabular-nums text-label-2" aria-hidden="true">
                      {line.newNo ?? ""}
                    </td>
                  </>
                )}
                <td
                  className={clsx(
                    "w-diff-sign select-none text-center",
                    line.kind === "add" ? "text-diff-add-fg" : "text-diff-del-fg",
                  )}
                >
                  {SIGN[line.kind] ?? ""}
                </td>
                <td className="pl-1 pr-4">{body || " "}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {hidden > 0 && <p className="px-3 pt-1 font-sans text-callout text-label-2">{hidden} more lines not shown</p>}
    </CodeFrame>
  );
}
