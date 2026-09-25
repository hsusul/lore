import { memo, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

import type { ActivityDto } from "../../ipc";
import { buildTimeline, openablePath, type TimelineEntry, type ToolCall } from "./activity";
import { highlight } from "./highlight";
import { ArrowDownIcon, FileIcon, GlobeIcon, PencilIcon, SearchIcon, TerminalIcon, ToolIcon } from "./icons";

const KIND_LABELS: Record<ActivityDto["kind"], string> = {
  message: "Message",
  tool: "Tool",
  result: "Result",
  error: "Error",
  output: "Output",
};

/** Output blocks longer than this collapse behind "Show all". */
export const OUTPUT_PREVIEW_LINES = 6;

type Props = {
  items: ActivityDto[] | null;
  error: string | null;
  compact?: boolean;
  label: string;
  /** The agent is working: show a live indicator at the end. */
  running?: boolean;
  /** Open a worktree-relative file named by a read or edit. */
  onOpenFile?: (relPath: string) => void;
};

/**
 * An agent's activity timeline. Stays pinned to the bottom while the reader is
 * already at the bottom; scrolling up stops the auto-scroll and offers a way back.
 */
export default function ActivityList({ items, error, compact = false, label, running = false, onOpenFile }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);
  const [detached, setDetached] = useState(false);
  const timeline = useMemo(() => (items && !compact ? buildTimeline(items) : null), [items, compact]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && stickRef.current) el.scrollTop = el.scrollHeight;
  }, [items, running]);

  function toLatest() {
    const el = scrollRef.current;
    if (!el) return;
    stickRef.current = true;
    setDetached(false);
    el.scrollTo?.({ top: el.scrollHeight, behavior: "smooth" });
  }

  return (
    <div className="activity-frame">
      <div
        ref={scrollRef}
        className={`activity${compact ? " activity--compact" : ""}`}
        onScroll={(e) => {
          const el = e.currentTarget;
          const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
          stickRef.current = atBottom;
          setDetached(!atBottom);
        }}
      >
        {error && (
          <p className="wb-note wb-note--error" role="alert">
            {error}
          </p>
        )}
        {items === null ? (
          !error && (
            <p className="wb-note" role="status">
              Loading activity…
            </p>
          )
        ) : items.length === 0 && !running ? (
          <p className="wb-note">No activity yet.</p>
        ) : compact || !timeline ? (
          <ol className="activity__list" aria-label={label}>
            {items.map((item, i) => (
              <li key={i} className={`activity__item activity__item--${item.kind}`}>
                <span className="visually-hidden">{KIND_LABELS[item.kind]}: </span>
                {item.text}
              </li>
            ))}
          </ol>
        ) : (
          <ol className="activity__list" aria-label={label}>
            {timeline.map((entry, i) => (
              <Entry key={i} entry={entry} onOpenFile={onOpenFile} />
            ))}
            {running && (
              <li className="activity__working" role="status">
                <span className="activity__working-dots" aria-hidden="true">
                  <i />
                  <i />
                  <i />
                </span>
                <span className="activity__shimmer">Working…</span>
              </li>
            )}
          </ol>
        )}
      </div>
      {detached && !compact && (
        <button type="button" className="activity__latest" onClick={toLatest}>
          <ArrowDownIcon /> Latest
        </button>
      )}
    </div>
  );
}

/** One timeline entry. */
function EntryView({
  entry,
  onOpenFile,
}: {
  entry: TimelineEntry;
  onOpenFile?: (relPath: string) => void;
}) {
  switch (entry.kind) {
    case "run":
      return (
        <li className="activity__run" aria-label={`Run ${entry.run}${entry.agent ? `, ${entry.agent}` : ""}`}>
          <span>
            Run {entry.run}
            {entry.agent && ` · ${entry.agent}`}
          </span>
        </li>
      );
    case "prompt":
      return (
        <li className="activity__prompt">
          <span className="visually-hidden">Prompt: </span>
          <span className="activity__bubble">{entry.text}</span>
        </li>
      );
    case "tool":
      return (
        <li className={`activity__item activity__item--tool activity__tool--${entry.tool.family}`}>
          <span className="visually-hidden">Tool: </span>
          <ToolGlyph family={entry.tool.family} />
          <span className="activity__tool-name">{entry.tool.name}</span> <ToolTarget tool={entry.tool} onOpenFile={onOpenFile} />
        </li>
      );
    case "output":
      return (
        <li className="activity__item activity__item--output">
          <span className="visually-hidden">Output: </span>
          <OutputBlock lines={entry.lines} />
        </li>
      );
    case "message":
    case "result":
      return (
        <li className={`activity__item activity__item--${entry.kind}`}>
          <span className="visually-hidden">{KIND_LABELS[entry.kind]}: </span>
          <div className="md">
            <Markdown remarkPlugins={[remarkGfm]} components={MARKDOWN}>
              {entry.text}
            </Markdown>
          </div>
        </li>
      );
    case "error":
      return (
        <li className="activity__item activity__item--error">
          <span className="visually-hidden">Error: </span>
          {entry.text}
        </li>
      );
  }
}

type EntryProps = Parameters<typeof EntryView>[0];

/**
 * The timeline is rebuilt on every poll, so entries compare by content: old ones
 * re-render (and re-parse their Markdown) only when they change.
 */
const Entry = memo(
  EntryView,
  (a: EntryProps, b: EntryProps) =>
    a.onOpenFile === b.onOpenFile && (a.entry === b.entry || JSON.stringify(a.entry) === JSON.stringify(b.entry)),
);

function ToolGlyph({ family }: { family: ToolCall["family"] }) {
  const glyph: Record<ToolCall["family"], ReactNode> = {
    read: <FileIcon />,
    edit: <PencilIcon />,
    shell: <TerminalIcon />,
    search: <SearchIcon size={14} />,
    web: <GlobeIcon />,
    other: <ToolIcon />,
  };
  return <span className="activity__tool-icon">{glyph[family]}</span>;
}

function ToolTarget({ tool, onOpenFile }: { tool: ToolCall; onOpenFile?: (relPath: string) => void }) {
  const path = onOpenFile ? openablePath(tool) : null;
  if (!tool.target) return null;
  if (path && onOpenFile) {
    return (
      <button type="button" className="activity__tool-target activity__tool-link" title={`Open ${path}`} onClick={() => onOpenFile(path)}>
        {tool.target}
      </button>
    );
  }
  return <span className="activity__tool-target">{tool.target}</span>;
}

function OutputBlock({ lines }: { lines: string[] }) {
  const [open, setOpen] = useState(false);
  const long = lines.length > OUTPUT_PREVIEW_LINES;
  const shown = long && !open ? lines.slice(-OUTPUT_PREVIEW_LINES) : lines;
  return (
    <>
      {long && (
        <button type="button" className="activity__more" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
          {open ? "Show less" : `Show ${lines.length - OUTPUT_PREVIEW_LINES} earlier lines`}
        </button>
      )}
      <pre className="activity__output">{shown.join("\n")}</pre>
    </>
  );
}

/**
 * Agent prose as Markdown. Links render as text with the URL in a tooltip: the
 * webview has no browser to hand them to, and navigating it would leave Lore.
 * Images show their alt text; remote loads are blocked by the CSP anyway.
 */
const MARKDOWN: Components = {
  a: ({ href, children }) => (
    <span className="md__link" title={href}>
      {children}
    </span>
  ),
  img: ({ alt }) => <span className="md__img">{alt ? `[${alt}]` : "[image]"}</span>,
  code: ({ className, children }) => {
    const lang = /language-(\w+)/.exec(className ?? "")?.[1];
    const text = String(children ?? "");
    if (!lang && !text.includes("\n")) return <code className="md__code">{children}</code>;
    return <code className={`md__block${lang ? ` language-${lang}` : ""}`}>{lang ? highlight(text, `x.${lang}`) : text}</code>;
  },
};
