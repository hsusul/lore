import { useEffect, useMemo, useRef, useState } from "react";

export type PaletteItem = {
  id: string;
  label: string;
  detail?: string;
  group: "Command" | "Agent" | "File";
  hint?: string;
  run: () => void;
};

/** With no query: commands, then agents, then files. */
const GROUP_ORDER: PaletteItem["group"][] = ["Command", "Agent", "File"];

const MAX_RESULTS = 100;

/**
 * Ordered fuzzy match: every query character must appear in order. Higher is
 * better; consecutive and word-start matches score more. Null when no match.
 */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return 0;
  const t = text.toLowerCase();
  let score = 0;
  let ti = 0;
  let prev = -2;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found < 0) return null;
    score += 1;
    if (found === prev + 1) score += 3;
    if (found === 0 || /[\s/._-]/.test(t[found - 1])) score += 2;
    prev = found;
    ti = found + 1;
  }
  return score - t.length * 0.01;
}

export function filterItems(items: PaletteItem[], query: string): PaletteItem[] {
  if (!query.trim()) {
    return GROUP_ORDER.flatMap((group) => items.filter((i) => i.group === group)).slice(0, MAX_RESULTS);
  }
  return items
    .map((item) => {
      const label = fuzzyScore(query, item.label);
      const detail = item.detail ? fuzzyScore(query, item.detail) : null;
      const best = label === null ? (detail === null ? null : detail - 1) : label;
      return { item, score: best };
    })
    .filter((x): x is { item: PaletteItem; score: number } => x.score !== null)
    .sort((a, b) => b.score - a.score)
    .slice(0, MAX_RESULTS)
    .map((x) => x.item);
}

type Props = {
  items: PaletteItem[];
  onClose: () => void;
  /** Launch an agent from what was typed; offered last whenever there is a query. */
  onNewAgent?: (prompt: string) => void;
};

/** ⌘K / ⌘P quick switcher over commands, agents, and files already loaded in the explorer. */
export default function CommandPalette({ items, onClose, onNewAgent }: Props) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    inputRef.current?.focus();
    return () => {
      if (previous && document.contains(previous)) previous.focus();
    };
  }, []);

  const results = useMemo(() => {
    const found = filterItems(items, query);
    const text = query.trim();
    if (!text || !onNewAgent) return found;
    return found.concat({
      id: "new-agent-from-query",
      group: "Command",
      label: `New agent: ${text}`,
      hint: "launch",
      run: () => onNewAgent(text),
    });
  }, [items, query, onNewAgent]);
  const current = Math.min(active, Math.max(results.length - 1, 0));

  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${current}"]`)
      ?.scrollIntoView?.({ block: "nearest" });
  }, [current]);

  function run(item: PaletteItem | undefined) {
    if (!item) return;
    onClose();
    item.run();
  }

  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div
        className="palette"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <input
          ref={inputRef}
          className="palette__input"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-autocomplete="list"
          aria-activedescendant={results.length ? `palette-opt-${current}` : undefined}
          aria-label="Search agents, commands, and files"
          placeholder="Search agents, commands, and files, or describe a new task"
          value={query}
          spellCheck={false}
          onChange={(e) => {
            setQuery(e.target.value);
            setActive(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setActive(results.length ? (current + 1) % results.length : 0);
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setActive(results.length ? (current - 1 + results.length) % results.length : 0);
            } else if (e.key === "Enter") {
              e.preventDefault();
              run(results[current]);
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              onClose();
            } else if (e.key === "Tab") {
              // A modal: focus stays in the search field.
              e.preventDefault();
            }
          }}
        />
        <ul id="palette-list" ref={listRef} className="palette__list" role="listbox" aria-label="Results">
          {results.length === 0 && <li className="palette__empty">No matching agents, commands, or files</li>}
          {results.map((item, i) => (
            <li
              key={item.id}
              id={`palette-opt-${i}`}
              data-index={i}
              role="option"
              aria-selected={i === current}
              className={`palette__item${i === current ? " palette__item--active" : ""}`}
              onMouseMove={() => i !== current && setActive(i)}
              onClick={() => run(item)}
            >
              <span className="palette__label">{item.label}</span>
              {item.detail && <span className="palette__detail">{item.detail}</span>}
              <span className="palette__hint">{item.hint ?? (item.group === "Command" ? "" : item.group.toLowerCase())}</span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
