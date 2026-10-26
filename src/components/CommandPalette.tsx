import { useEffect, useMemo, useRef, useState } from "react";

import { Modal } from "./Modal";

export interface Command {
  id: string;
  group: string;
  label: string;
  hint?: string;
  run: () => void;
}

const MAX_SHOWN = 50;
const ARCHIVE_DEBOUNCE_MS = 160;
const ARCHIVE_MIN_LENGTH = 2;

/**
 * Score an ordered fuzzy match: substrings beat scattered letters, word starts
 * beat word middles, and wide gaps cost points. `null` means no match.
 */
export function fuzzyScore(query: string, candidate: string): number | null {
  const needle = query.trim().toLowerCase();
  const hay = candidate.toLowerCase();
  if (needle === "") return 0;
  const at = hay.indexOf(needle);
  if (at >= 0) return 1_000 - at * 2 - (hay.length - needle.length) * 0.1;
  let score = 0;
  let prev = -1;
  for (const ch of needle) {
    const i = hay.indexOf(ch, prev + 1);
    if (i < 0) return null;
    score += i === 0 || /[\s/_.:-]/.test(hay[i - 1] ?? "") ? 16 : 8;
    if (prev >= 0) score += i === prev + 1 ? 12 : Math.max(0, 6 - (i - prev - 1));
    prev = i;
  }
  return score - (hay.length - needle.length) * 0.1;
}

function scoreCommand(query: string, command: Command): number | null {
  const label = fuzzyScore(query, command.label);
  const hint = command.hint ? fuzzyScore(query, command.hint) : null;
  if (label === null && hint === null) return null;
  return Math.max(label ?? -Infinity, hint === null ? -Infinity : hint - 30);
}

type ArchiveState =
  | { status: "idle" }
  | { status: "loading"; query: string }
  | { status: "ready"; query: string; items: Command[] }
  | { status: "failed"; query: string };

/**
 * Jump anywhere: actions, repositories, folders, loaded sessions, and a
 * full-text search of the archive for anything else.
 */
export default function CommandPalette({
  commands,
  searchArchive,
  onClose,
}: {
  commands: Command[];
  searchArchive: (query: string) => Promise<Command[]>;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [archive, setArchive] = useState<ArchiveState>({ status: "idle" });
  const input = useRef<HTMLInputElement>(null);
  const trimmed = query.trim();

  useEffect(() => {
    if (trimmed.length < ARCHIVE_MIN_LENGTH) {
      setArchive({ status: "idle" });
      return;
    }
    let cancelled = false;
    setArchive({ status: "loading", query: trimmed });
    const timer = setTimeout(() => {
      searchArchive(trimmed).then(
        (items) => !cancelled && setArchive({ status: "ready", query: trimmed, items }),
        () => !cancelled && setArchive({ status: "failed", query: trimmed }),
      );
    }, ARCHIVE_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [searchArchive, trimmed]);

  const shown = useMemo(() => {
    if (trimmed === "") return commands.slice(0, MAX_SHOWN);
    const local = commands
      .map((command, index) => ({ command, index, score: scoreCommand(trimmed, command) }))
      .filter((m): m is typeof m & { score: number } => m.score !== null);
    // Full-text hits can match a session body even when the title does not
    // fuzzy-match, so they rank after direct matches instead of disappearing.
    const remote =
      archive.status === "ready" && archive.query === trimmed
        ? archive.items.map((command, index) => ({
            command,
            index: commands.length + index,
            score: scoreCommand(trimmed, command) ?? -100 - index,
          }))
        : [];
    const best = new Map<string, (typeof local)[number]>();
    for (const match of [...local, ...remote]) {
      const seen = best.get(match.command.id);
      if (!seen || match.score > seen.score) best.set(match.command.id, match);
    }
    return [...best.values()]
      .sort((a, b) => b.score - a.score || a.index - b.index)
      .slice(0, MAX_SHOWN)
      .map((m) => m.command);
  }, [archive, commands, trimmed]);

  const current = Math.min(active, Math.max(shown.length - 1, 0));

  useEffect(() => {
    document.getElementById(shown[current]?.id ?? "")?.scrollIntoView?.({ block: "nearest" });
  }, [current, shown]);

  function run(command: Command | undefined) {
    if (!command) return;
    onClose();
    command.run();
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown") setActive(Math.min(current + 1, shown.length - 1));
    else if (event.key === "ArrowUp") setActive(Math.max(current - 1, 0));
    else if (event.key === "Home") setActive(0);
    else if (event.key === "End") setActive(Math.max(shown.length - 1, 0));
    else if (event.key === "Enter") run(shown[current]);
    else return;
    event.preventDefault();
  }

  const searching = archive.status === "loading";
  return (
    <Modal label="Command palette" className="palette" onClose={onClose} initialFocus={input}>
      <input
        ref={input}
        className="palette__input"
        type="text"
        maxLength={256}
        role="combobox"
        aria-label="Jump to a session, repository, folder, or action"
        aria-expanded="true"
        aria-controls="palette-list"
        aria-activedescendant={shown[current]?.id}
        placeholder="Jump to a session, repository, folder, or action"
        value={query}
        onChange={(event) => {
          setQuery(event.target.value);
          setActive(0);
        }}
        onKeyDown={onKeyDown}
      />
      <ul id="palette-list" className="palette__list" role="listbox" aria-label="Results" aria-busy={searching}>
        {shown.map((command, index) => (
          <li
            key={command.id}
            id={command.id}
            role="option"
            aria-selected={index === current}
            className={`palette__item${index === current ? " is-active" : ""}`}
            onMouseEnter={() => setActive(index)}
            onClick={() => run(command)}
          >
            <span className="palette__label">{command.label}</span>
            {command.hint && <span className="palette__hint">{command.hint}</span>}
            <span className="palette__group">{command.group}</span>
          </li>
        ))}
      </ul>
      <p className="palette__status" role="status" aria-live="polite">
        {shown.length === 0 && !searching
          ? `Nothing matches “${trimmed}”.`
          : searching
            ? "Searching the archive…"
            : archive.status === "failed"
              ? "Archive search failed. Local matches are still shown."
              : ""}
      </p>
    </Modal>
  );
}
