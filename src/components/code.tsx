import { clsx } from "clsx";
import { Copy } from "lucide-react";
import type { ReactNode } from "react";

import { useNote } from "../hooks/useNote";
import { Highlight } from "./Highlight";
import { Button, ICON } from "./ui/button";
import { Tooltip } from "./ui/tooltip";

const MINUS = "−";

/** Copies text; its tooltip says "Copied" in place of a toast (spec 9.12). */
export function CopyButton({ text, label }: { text: string; label: string }) {
  const [note, show] = useNote();
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      show("Copied");
    } catch {
      show("Could Not Copy");
    }
  }
  return (
    <Tooltip label={label} note={note}>
      <Button variant="icon" aria-label={label} onClick={() => void copy()}>
        <Copy className={ICON.small} aria-hidden="true" />
      </Button>
    </Tooltip>
  );
}

/**
 * A code, patch or output block (spec 9.5): the content color with a 0.5px ring, 10px
 * radius, horizontal scroll, and an optional one-line header that sticks to the top of the
 * reader. The header sits outside the scrolling body so it can stick.
 */
export function CodeFrame({
  label,
  header,
  failed = false,
  bounded = false,
  children,
}: {
  label?: string;
  header?: ReactNode;
  failed?: boolean;
  bounded?: boolean;
  children: ReactNode;
}) {
  return (
    <figure
      aria-label={label}
      className={clsx("min-w-0 rounded-group bg-bg-content shadow-ring", failed && "border-l-edge border-red")}
    >
      {header && (
        <figcaption className="sticky top-0 z-10 flex h-tool-row min-w-0 items-center gap-1.5 rounded-t-group bg-bg-content pl-3 pr-0.5 text-callout shadow-hairline-b">
          {header}
        </figcaption>
      )}
      <div
        className={clsx(
          "overflow-auto py-2 font-mono text-code",
          header ? "rounded-b-group" : "rounded-group",
          bounded && "max-h-output",
        )}
      >
        {children}
      </div>
    </figure>
  );
}

/**
 * A path in mono, truncated in the middle as macOS does: the directory in secondary text
 * gives way first, the file name in primary text stays.
 */
export function PathText({ path, terms = [], className }: { path: string; terms?: string[]; className?: string }) {
  const slash = path.lastIndexOf("/");
  return (
    <span
      className={clsx("flex min-w-0 overflow-hidden whitespace-nowrap font-mono text-callout", className)}
      title={path}
      aria-label={path}
    >
      {slash >= 0 && (
        <span className="min-w-0 truncate text-label-2">
          <Highlight text={path.slice(0, slash + 1)} terms={terms} />
        </span>
      )}
      <span className="flex-none text-label">
        <Highlight text={path.slice(slash + 1)} terms={terms} />
      </span>
    </span>
  );
}

/** "+3 −1": in the diff colors, or quiet as a tool row's trailing summary. */
export function Counts({ added, removed, quiet = false }: { added: number | null; removed: number | null; quiet?: boolean }) {
  if (added === null && removed === null) return null;
  if (quiet) {
    const text = [added !== null && `+${added}`, removed !== null && `${MINUS}${removed}`].filter(Boolean).join(" ");
    return <span className="flex-none text-callout tabular-nums text-label-2">{text}</span>;
  }
  return (
    <span className="flex-none text-callout tabular-nums">
      {added !== null && <span className="text-diff-add-fg">+{added}</span>}
      {added !== null && removed !== null && " "}
      {removed !== null && (
        <span className="text-diff-del-fg">
          {MINUS}
          {removed}
        </span>
      )}
    </span>
  );
}
