import { useEffect, useMemo, useRef, useState } from "react";

export interface Command {
  id: string;
  label: string;
  hint?: string;
  group?: string;
  run: () => void;
}

const MAX_RESULTS = 50;

/**
 * A ⌘K command palette (Raycast/Linear pattern): fuzzy-filter over
 * repositories, sessions, and actions, keyboard-driven (↑/↓, Enter, Esc).
 */
export default function CommandPalette({
  items,
  open,
  onClose,
}: {
  items: Command[];
  open: boolean;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const matches = q
      ? items.filter(
          (item) =>
            item.label.toLowerCase().includes(q) ||
            item.hint?.toLowerCase().includes(q) ||
            item.group?.toLowerCase().includes(q),
        )
      : items;
    return matches.slice(0, MAX_RESULTS);
  }, [items, query]);

  useEffect(() => {
    if (open) {
      setQuery("");
      setActive(0);
      inputRef.current?.focus();
    }
  }, [open]);

  useEffect(() => setActive(0), [query]);

  if (!open) return null;

  function run(index: number) {
    const command = filtered[index];
    if (command) {
      command.run();
      onClose();
    }
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((a) => Math.min(a + 1, filtered.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      run(active);
    }
  }

  return (
    <div className="palette__backdrop" role="presentation" onClick={onClose}>
      <div
        className="palette"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onClick={(event) => event.stopPropagation()}
      >
        <input
          ref={inputRef}
          className="palette__input"
          placeholder="Jump to a repository or session…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={onKeyDown}
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={filtered[active]?.id}
        />
        <ul id="palette-list" className="palette__list" role="listbox">
          {filtered.length === 0 ? (
            <li className="palette__empty">No matches</li>
          ) : (
            filtered.map((command, index) => (
              <li
                key={command.id}
                id={command.id}
                role="option"
                aria-selected={index === active}
                className={`palette__item${index === active ? " is-active" : ""}`}
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
      </div>
    </div>
  );
}
