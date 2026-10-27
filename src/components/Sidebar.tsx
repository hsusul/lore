import { clsx } from "clsx";
import { ChevronDown, PanelLeft, Plus } from "lucide-react";
import { useRef, useState } from "react";

import type { FolderSummary } from "../ipc";
import { sameSource, type Source } from "../hooks/workspace";
import { Button, ICON } from "./ui/button";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from "./ui/context-menu";
import { Tooltip } from "./ui/tooltip";
import type { SourceRow, SourceSection } from "./sources";

/** The toolbar button that shows and hides the sidebar (⌃⌘S). */
export function SidebarToggle({ visible, onToggle }: { visible: boolean; onToggle: () => void }) {
  const label = visible ? "Hide Sidebar" : "Show Sidebar";
  return (
    <Tooltip label={`${label} (⌃⌘S)`}>
      <Button variant="toolbar" className="ml-2" aria-label={label} aria-keyshortcuts="Control+Meta+S" onClick={onToggle}>
        <PanelLeft className={ICON.toolbar} aria-hidden="true" />
      </Button>
    </Tooltip>
  );
}

export interface FolderActions {
  onNewFolder: () => void;
  onRenameFolder: (folder: FolderSummary) => void;
  onDeleteFolder: (folder: FolderSummary) => void;
}

/**
 * The source list (spec 9.1), Mail's mailbox column: All Sessions, then Repositories,
 * Folders and Agents with counts. Selection is a filled inset row, accent-blue while the
 * sidebar has focus and gray otherwise. Arrow keys move the selection, as in Mail.
 */
export default function Sidebar({
  sections,
  selected,
  onChoose,
  onToggle,
  folderActions,
}: {
  sections: SourceSection[];
  selected: Source;
  onChoose: (source: Source) => void;
  onToggle: () => void;
  folderActions: FolderActions;
}) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  const visibleRows = sections.flatMap((s) => (s.title && collapsed.has(s.title) ? [] : s.rows));
  const selectedIndex = visibleRows.findIndex((r) => sameSource(r.source, selected));
  const tabStop = Math.max(selectedIndex, 0);

  function toggleSection(title: string) {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (!next.delete(title)) next.add(title);
      return next;
    });
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const from = buttons.current.indexOf(event.target as HTMLButtonElement);
    if (from < 0) return;
    const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    const to = from + step;
    if (step === 0 || to < 0 || to >= visibleRows.length) return;
    event.preventDefault();
    buttons.current[to]?.focus();
    onChoose(visibleRows[to].source);
  }

  let index = 0;
  return (
    <nav aria-label="Sidebar" className="group/sidebar grid min-h-0 min-w-0 grid-rows-sidebar bg-bg-sidebar shadow-hairline-r select-none">
      <div data-tauri-drag-region className="flex items-center pl-traffic-zone">
        <SidebarToggle visible onToggle={onToggle} />
      </div>
      <div className="min-h-0 overflow-y-auto pb-2" onKeyDown={onKeyDown}>
        {sections.map((section) => {
          const open = !section.title || !collapsed.has(section.title);
          return (
            <section key={section.title ?? "library"} aria-label={section.title ?? "Library"}>
              {section.title && (
                <SectionHeader title={section.title} open={open} onToggle={() => toggleSection(section.title as string)} />
              )}
              {open && (
                <ul>
                  {section.rows.map((row) => {
                    const at = index++;
                    return (
                      <li key={row.key} className="px-2.5">
                        <SourceButton
                          row={row}
                          buttonRef={(el) => {
                            buttons.current[at] = el;
                          }}
                          selected={at === selectedIndex}
                          tabbable={at === tabStop}
                          onChoose={onChoose}
                          folderActions={folderActions}
                        />
                      </li>
                    );
                  })}
                </ul>
              )}
            </section>
          );
        })}
      </div>
      <div className="px-2.5 pb-2.5 pt-1">
        <button
          type="button"
          className="flex h-source-row items-center gap-1.5 rounded-control px-2 text-body text-label-2 outline-none hover:text-label focus-visible:shadow-focus"
          onClick={folderActions.onNewFolder}
        >
          <Plus className={ICON.body} aria-hidden="true" />
          New Folder
        </button>
      </div>
    </nav>
  );
}

function SectionHeader({ title, open, onToggle }: { title: string; open: boolean; onToggle: () => void }) {
  return (
    <h2 className="mx-2.5 mb-1 mt-5">
      <button
        type="button"
        aria-expanded={open}
        className="group/header flex w-full items-center gap-1 rounded-control px-2 text-subheadline font-semibold text-label-2 outline-none focus-visible:shadow-focus"
        onClick={onToggle}
      >
        {title}
        <ChevronDown
          className={clsx(
            ICON.small,
            "ml-auto opacity-0 transition-transform duration-disclosure ease-out group-hover/header:opacity-100 group-focus-visible/header:opacity-100",
            !open && "-rotate-90",
          )}
          aria-hidden="true"
        />
      </button>
    </h2>
  );
}

function SourceButton({
  row,
  buttonRef,
  selected,
  tabbable,
  onChoose,
  folderActions,
}: {
  row: SourceRow;
  buttonRef: (el: HTMLButtonElement | null) => void;
  selected: boolean;
  tabbable: boolean;
  onChoose: (source: Source) => void;
  folderActions: FolderActions;
}) {
  const Icon = row.icon;
  const onSelected = "group-focus-within/sidebar:text-on-selected";
  const button = (
    <button
      type="button"
      ref={buttonRef}
      tabIndex={tabbable ? 0 : -1}
      aria-current={selected ? "true" : undefined}
      aria-label={row.note ? `${row.label}, ${row.note}` : undefined}
      title={row.note ?? undefined}
      className={clsx(
        "flex h-source-row w-full items-center gap-1.5 rounded-control px-2 text-body outline-none focus-visible:shadow-focus-inset",
        row.missing ? "text-label-3" : "text-label",
        selected && ["bg-selected-unfocused group-focus-within/sidebar:bg-selected group-focus-within/sidebar:focus-visible:shadow-none", onSelected],
      )}
      onClick={() => onChoose(row.source)}
    >
      <Icon className={clsx(ICON.body, row.missing ? "text-label-3" : "text-accent", selected && onSelected)} aria-hidden="true" />
      <span className="min-w-0 flex-1 truncate text-left">{row.label}</span>
      {row.count !== null && (
        <span
          className={clsx(
            "text-subheadline tabular-nums",
            row.missing ? "text-label-3" : selected ? ["text-label", onSelected] : "text-label-2",
          )}
        >
          {row.count}
        </span>
      )}
    </button>
  );
  if (!row.folder) return button;
  const folder = row.folder;
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{button}</ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onSelect={() => folderActions.onRenameFolder(folder)}>Rename Folder…</ContextMenuItem>
        <ContextMenuItem onSelect={() => folderActions.onDeleteFolder(folder)}>Delete Folder…</ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={folderActions.onNewFolder}>New Folder…</ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
