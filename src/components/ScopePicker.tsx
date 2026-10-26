import { useEffect, useMemo, useRef, useState } from "react";

import type { FolderSummary, RepositorySummary } from "../ipc";
import { sameScope, type Scope } from "../hooks/workspace";

interface Choice {
  id: string;
  scope: Scope;
  group: "Archive" | "Repositories" | "Folders";
  label: string;
  count: number | null;
  note: string | null;
}

/**
 * Pick what the list browses: everything, one repository, or one folder.
 * Type to narrow, arrows to move, Enter to choose, Escape to close.
 */
export default function ScopePicker({
  scope,
  total,
  repositories,
  folders,
  onChoose,
  onNewFolder,
  onClose,
}: {
  scope: Scope;
  total: number | null;
  repositories: RepositorySummary[];
  folders: FolderSummary[];
  onChoose: (scope: Scope) => void;
  onNewFolder: () => void;
  onClose: () => void;
}) {
  const [filter, setFilter] = useState("");
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const root = useRef<HTMLDivElement>(null);

  const choices = useMemo<Choice[]>(() => {
    const all: Choice[] = [
      { id: "scope-all", scope: { kind: "all" }, group: "Archive", label: "All sessions", count: total, note: null },
      ...repositories.map((r) => ({
        id: `scope-repo-${r.id}`,
        scope: { kind: "repository", id: r.id } as Scope,
        group: "Repositories" as const,
        label: r.display_name,
        count: r.session_count,
        note: r.is_missing ? "checkout missing" : r.identity_confidence === "low" ? "low confidence" : null,
      })),
      ...folders.map((f) => ({
        id: `scope-folder-${f.id}`,
        scope: { kind: "folder", id: f.id } as Scope,
        group: "Folders" as const,
        label: f.name,
        count: f.session_count,
        note: null,
      })),
    ];
    const needle = filter.trim().toLowerCase();
    return needle === "" ? all : all.filter((c) => c.label.toLowerCase().includes(needle));
  }, [filter, folders, repositories, total]);

  const current = Math.min(active, Math.max(choices.length - 1, 0));

  useEffect(() => {
    input.current?.focus();
  }, []);

  useEffect(() => {
    function onPointer(event: MouseEvent) {
      if (root.current && !root.current.contains(event.target as Node)) onClose();
    }
    document.addEventListener("mousedown", onPointer);
    return () => document.removeEventListener("mousedown", onPointer);
  }, [onClose]);

  useEffect(() => {
    document.getElementById(choices[current]?.id ?? "")?.scrollIntoView?.({ block: "nearest" });
  }, [choices, current]);

  function onKeyDown(event: React.KeyboardEvent) {
    if (event.key === "ArrowDown") setActive(Math.min(current + 1, choices.length - 1));
    else if (event.key === "ArrowUp") setActive(Math.max(current - 1, 0));
    else if (event.key === "Enter") {
      const choice = choices[current];
      if (choice) onChoose(choice.scope);
    } else if (event.key === "Escape") onClose();
    else return;
    event.preventDefault();
  }

  let lastGroup: string | null = null;
  return (
    <div ref={root} className="popover scope" role="dialog" aria-label="Choose what to browse">
      <input
        ref={input}
        className="popover__input"
        type="text"
        role="combobox"
        aria-expanded="true"
        aria-controls="scope-list"
        aria-activedescendant={choices[current]?.id}
        aria-label="Filter repositories and folders"
        placeholder="Repositories and folders"
        value={filter}
        onChange={(event) => {
          setFilter(event.target.value);
          setActive(0);
        }}
        onKeyDown={onKeyDown}
      />
      <ul id="scope-list" className="popover__list" role="listbox" aria-label="Repositories and folders">
        {choices.length === 0 && <li className="popover__empty">Nothing matches “{filter.trim()}”.</li>}
        {choices.map((choice, index) => {
          const heading = choice.group !== lastGroup ? choice.group : null;
          lastGroup = choice.group;
          return (
            <li key={choice.id} role="presentation">
              {heading && heading !== "Archive" && (
                <div className="popover__group" aria-hidden="true">
                  {heading}
                </div>
              )}
              <div
                id={choice.id}
                role="option"
                aria-selected={sameScope(choice.scope, scope)}
                aria-label={
                  choice.group === "Archive"
                    ? undefined
                    : `${choice.label}, ${choice.group === "Folders" ? "folder" : "repository"}${choice.note ? `, ${choice.note}` : ""}`
                }
                className={`popover__item${index === current ? " is-active" : ""}`}
                onMouseEnter={() => setActive(index)}
                onClick={() => onChoose(choice.scope)}
              >
                <span className="popover__label">{choice.label}</span>
                {choice.note && <span className="flag flag--warn">{choice.note}</span>}
                {choice.count !== null && <span className="count">{choice.count}</span>}
              </div>
            </li>
          );
        })}
      </ul>
      <div className="popover__foot">
        <button type="button" className="btn btn--quiet" onClick={onNewFolder}>
          New folder…
        </button>
        {repositories.length === 0 && <span className="quiet">Repositories appear after a scan finds git evidence.</span>}
      </div>
    </div>
  );
}
