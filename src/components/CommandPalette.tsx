import { useEffect, useMemo, useRef, useState } from "react";

import { useFocusTrap } from "../focus-trap";

export interface Command {
  id: string;
  label: string;
  hint?: string;
  group?: string;
  run: () => void;
}

const MAX_RESULTS = 50;
const ARCHIVE_SEARCH_DEBOUNCE_MS = 160;
const ARCHIVE_SEARCH_MIN_LENGTH = 2;

type ArchiveSearch = (query: string) => Promise<Command[]>;

interface ArchiveState {
  query: string;
  items: Command[];
  status: "idle" | "loading" | "settled" | "failed";
}

const IDLE_ARCHIVE: ArchiveState = { query: "", items: [], status: "idle" };

/**
 * Rank a compact, ordered fuzzy match. Contiguous and word-boundary matches
 * win, while wide gaps lose points. `null` means the query is not a
 * subsequence of the candidate at all.
 */
function fuzzyScore(query: string, candidate: string): number | null {
  const needle = query.trim().toLowerCase();
  const haystack = candidate.toLowerCase();
  if (needle === "") return 0;

  const substringAt = haystack.indexOf(needle);
  if (substringAt >= 0) {
    return 1_000 - substringAt * 2 - (haystack.length - needle.length) * 0.1;
  }

  let score = 0;
  let previous = -1;
  let first = -1;
  for (const character of needle) {
    const at = haystack.indexOf(character, previous + 1);
    if (at < 0) return null;
    if (first < 0) first = at;

    const atBoundary = at === 0 || /[\s/_.:-]/.test(haystack[at - 1] ?? "");
    score += atBoundary ? 16 : 8;
    if (previous >= 0) {
      const gap = at - previous - 1;
      score += gap === 0 ? 12 : Math.max(0, 6 - gap);
    }
    previous = at;
  }

  return score - first - (haystack.length - needle.length) * 0.1;
}

function commandScore(query: string, command: Command): number | null {
  const label = fuzzyScore(query, command.label);
  const hint = command.hint ? fuzzyScore(query, command.hint) : null;
  const group = command.group ? fuzzyScore(query, command.group) : null;
  const scores = [
    label,
    hint === null ? null : hint - 30,
    group === null ? null : group - 50,
  ].filter((score): score is number => score !== null);
  return scores.length > 0 ? Math.max(...scores) : null;
}

/**
 * A ⌘K command palette (Raycast/Linear pattern): fuzzy-filter over
 * repositories, sessions, and actions, keyboard-driven (↑/↓, Enter, Esc).
 */
export default function CommandPalette({
  items,
  search,
  onClose,
}: {
  items: Command[];
  search?: ArchiveSearch;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [archive, setArchive] = useState<ArchiveState>(IDLE_ARCHIVE);
  const inputRef = useRef<HTMLInputElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const activeOptionRef = useRef<HTMLLIElement>(null);
  const archiveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const archiveRequestRef = useRef(0);
  const searchRef = useRef(search);
  useFocusTrap(dialogRef, true);

  const filtered = useMemo(() => {
    const q = query.trim();
    if (!q) return items.slice(0, MAX_RESULTS);
    const localMatches = items
      .map((item, index) => ({ item, index, score: commandScore(q, item) }))
      .filter((match): match is typeof match & { score: number } => match.score !== null);
    const archiveMatches = archive.query === q
      ? archive.items.map((item, index) => ({
          item,
          index: items.length + index,
          // FTS can match a session body even when its title does not fuzzy
          // match. Keep those matches, after direct command-label matches.
          score: commandScore(q, item) ?? -100 - index,
        }))
      : [];
    const deduplicated = new Map<string, (typeof localMatches)[number]>();
    for (const match of [...localMatches, ...archiveMatches]) {
      const previous = deduplicated.get(match.item.id);
      if (!previous || match.score > previous.score) deduplicated.set(match.item.id, match);
    }
    return [...deduplicated.values()]
      .sort((a, b) => b.score - a.score || a.index - b.index)
      .slice(0, MAX_RESULTS)
      .map(({ item }) => item);
  }, [archive, items, query]);

  const activeIndex = filtered.length === 0 ? 0 : Math.min(active, filtered.length - 1);
  const archiveLoading = archive.query === query.trim() && archive.status === "loading";
  const archiveFailed = archive.query === query.trim() && archive.status === "failed";

  useEffect(() => {
    searchRef.current = search;
  }, [search]);

  useEffect(() => {
    const previousFocus =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    inputRef.current?.focus();
    return () => {
      if (previousFocus?.isConnected) previousFocus.focus();
    };
  }, []);

  useEffect(
    () => () => {
      archiveRequestRef.current += 1;
      if (archiveTimerRef.current !== null) clearTimeout(archiveTimerRef.current);
    },
    [],
  );

  useEffect(() => {
    activeOptionRef.current?.scrollIntoView?.({ block: "nearest" });
  }, [activeIndex, filtered]);

  function run(index: number) {
    const command = filtered[index];
    if (command) {
      command.run();
      onClose();
    }
  }

  function updateQuery(next: string) {
    setQuery(next);
    setActive(0);
    archiveRequestRef.current += 1;
    const request = archiveRequestRef.current;
    if (archiveTimerRef.current !== null) clearTimeout(archiveTimerRef.current);

    const normalized = next.trim();
    if (!searchRef.current || normalized.length < ARCHIVE_SEARCH_MIN_LENGTH) {
      setArchive(IDLE_ARCHIVE);
      return;
    }

    setArchive({ query: normalized, items: [], status: "loading" });
    archiveTimerRef.current = setTimeout(() => {
      archiveTimerRef.current = null;
      void searchRef.current?.(normalized).then(
        (nextItems) => {
          if (request !== archiveRequestRef.current) return;
          setArchive({ query: normalized, items: nextItems, status: "settled" });
        },
        () => {
          if (request !== archiveRequestRef.current) return;
          setArchive({ query: normalized, items: [], status: "failed" });
        },
      );
    }, ARCHIVE_SEARCH_DEBOUNCE_MS);
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      if (filtered.length > 0) setActive(Math.min(activeIndex + 1, filtered.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (filtered.length > 0) setActive(Math.max(activeIndex - 1, 0));
    } else if (event.key === "Home") {
      event.preventDefault();
      if (filtered.length > 0) setActive(0);
    } else if (event.key === "End") {
      event.preventDefault();
      if (filtered.length > 0) setActive(filtered.length - 1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      run(activeIndex);
    }
  }

  return (
    <div className="palette__backdrop" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="palette"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onClick={(event) => event.stopPropagation()}
      >
        <input
          ref={inputRef}
          type="text"
          className="palette__input"
          placeholder="Search actions, repositories, and sessions…"
          value={query}
          onChange={(event) => updateQuery(event.target.value)}
          onKeyDown={onKeyDown}
          role="combobox"
          aria-label="Search commands and sessions"
          aria-autocomplete="list"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={filtered[activeIndex]?.id}
        />
        <ul
          id="palette-list"
          className="palette__list"
          role="listbox"
          aria-busy={archiveLoading}
        >
          {filtered.length === 0 && !archiveLoading ? (
            <li className="palette__empty">No matches</li>
          ) : (
            filtered.map((command, index) => (
              <li
                ref={index === activeIndex ? activeOptionRef : undefined}
                key={command.id}
                id={command.id}
                role="option"
                aria-selected={index === activeIndex}
                className={`palette__item${index === activeIndex ? " is-active" : ""}`}
                onMouseEnter={() => setActive(index)}
                onClick={() => run(index)}
              >
                {command.group && <span className="palette__group">{command.group}</span>}
                <span className="palette__label">{command.label}</span>
                {command.hint && <span className="palette__hint">{command.hint}</span>}
              </li>
            ))
          )}
        </ul>
        {(archiveLoading || archiveFailed) && (
          <p className="palette__status" role="status" aria-live="polite">
            {archiveLoading ? "Searching archive…" : "Archive search unavailable."}
          </p>
        )}
      </div>
    </div>
  );
}
