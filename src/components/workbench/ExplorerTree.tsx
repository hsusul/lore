import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";

import type { DirEntryDto } from "../../ipc";
import { ChevronIcon, FileIcon, FolderIcon } from "./icons";
import { dirKey, type DirCache } from "./useDirCache";

type Row = { entry: DirEntryDto; level: number; parent: string };

type Props = {
  root: string;
  cache: DirCache;
  /** Relative paths of expanded folders for this root. */
  expanded: Set<string>;
  onToggle: (relPath: string, open: boolean) => void;
  onOpenFile: (relPath: string) => void;
  activeRelPath: string | null;
};

/** Lazy file tree for one scope root, following the ARIA treeview pattern. */
export default function ExplorerTree({ root, cache, expanded, onToggle, onOpenFile, activeRelPath }: Props) {
  const { entries, errors, load } = cache;
  const [focused, setFocused] = useState<string | null>(null);
  const itemRefs = useRef(new Map<string, HTMLLIElement>());
  const treeRef = useRef<HTMLUListElement>(null);

  useEffect(() => {
    void load(root, "");
  }, [root, load]);

  const rows = useMemo(() => {
    const out: Row[] = [];
    const walk = (rel: string, level: number) => {
      for (const entry of entries[dirKey(root, rel)] ?? []) {
        out.push({ entry, level, parent: rel });
        if (entry.is_dir && expanded.has(entry.rel_path)) walk(entry.rel_path, level + 1);
      }
    };
    walk("", 1);
    return out;
  }, [entries, root, expanded]);

  const focusIndex = Math.max(
    0,
    rows.findIndex((r) => r.entry.rel_path === focused),
  );

  function moveFocus(relPath: string) {
    setFocused(relPath);
    itemRefs.current.get(relPath)?.focus();
  }

  function activate(row: Row) {
    if (row.entry.is_dir) {
      const open = !expanded.has(row.entry.rel_path);
      onToggle(row.entry.rel_path, open);
      if (open) void load(root, row.entry.rel_path);
    } else {
      onOpenFile(row.entry.rel_path);
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLUListElement>) {
    const row = rows[focusIndex];
    if (!row) return;
    const { entry } = row;
    switch (event.key) {
      case "ArrowDown":
        if (rows[focusIndex + 1]) moveFocus(rows[focusIndex + 1].entry.rel_path);
        break;
      case "ArrowUp":
        if (focusIndex > 0) moveFocus(rows[focusIndex - 1].entry.rel_path);
        break;
      case "Home":
        moveFocus(rows[0].entry.rel_path);
        break;
      case "End":
        moveFocus(rows[rows.length - 1].entry.rel_path);
        break;
      case "ArrowRight":
        if (!entry.is_dir) break;
        if (!expanded.has(entry.rel_path)) {
          onToggle(entry.rel_path, true);
          void load(root, entry.rel_path);
        } else if (rows[focusIndex + 1]?.parent === entry.rel_path) {
          moveFocus(rows[focusIndex + 1].entry.rel_path);
        }
        break;
      case "ArrowLeft":
        if (entry.is_dir && expanded.has(entry.rel_path)) onToggle(entry.rel_path, false);
        else if (row.parent !== "") moveFocus(row.parent);
        break;
      case "Enter":
      case " ":
        activate(row);
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  const rootKey = dirKey(root, "");
  if (errors[rootKey]) {
    return (
      <p className="wb-note wb-note--error" role="alert">
        {errors[rootKey]}
      </p>
    );
  }
  if (!entries[rootKey]) {
    return (
      <p className="wb-note" role="status">
        Loading…
      </p>
    );
  }
  if (rows.length === 0) return <p className="wb-note">This folder is empty.</p>;

  return (
    <ul className="tree" role="tree" aria-label="Files" ref={treeRef} onKeyDown={onKeyDown}>
      {rows.map((row, i) => {
        const { entry, level } = row;
        const open = entry.is_dir && expanded.has(entry.rel_path);
        const key = dirKey(root, entry.rel_path);
        const loading = open && !entries[key] && !errors[key];
        return (
          <li
            key={entry.rel_path}
            ref={(el) => {
              if (el) itemRefs.current.set(entry.rel_path, el);
              else itemRefs.current.delete(entry.rel_path);
            }}
            role="treeitem"
            aria-level={level}
            aria-expanded={entry.is_dir ? open : undefined}
            aria-selected={entry.rel_path === activeRelPath}
            aria-busy={loading || undefined}
            tabIndex={i === focusIndex ? 0 : -1}
            title={entry.rel_path}
            className={`tree__row${entry.rel_path === activeRelPath ? " tree__row--active" : ""}`}
            style={{ paddingLeft: 4 + (level - 1) * 12 }}
            onFocus={() => setFocused(entry.rel_path)}
            onClick={() => {
              setFocused(entry.rel_path);
              activate(row);
            }}
          >
            <span className={`tree__twisty${open ? " tree__twisty--open" : ""}`}>
              {entry.is_dir && <ChevronIcon />}
            </span>
            <span className={`tree__icon${entry.is_dir ? " tree__icon--dir" : ""}`}>
              {entry.is_dir ? <FolderIcon /> : <FileIcon />}
            </span>
            <span className="tree__label">{entry.name}</span>
            {errors[key] && (
              <span className="tree__error" title={errors[key]}>
                failed to load
              </span>
            )}
          </li>
        );
      })}
    </ul>
  );
}
