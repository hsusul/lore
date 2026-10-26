import { clsx } from "clsx";
import { ChevronDown } from "lucide-react";
import type { ReactNode } from "react";

import type { Source } from "../hooks/workspace";
import { ICON } from "./ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  MenuTrailing,
} from "./ui/dropdown-menu";
import type { SourceSection } from "./sources";

/**
 * The unified toolbar (spec 7.1), split at the column edges like Mail's: one segment over
 * the session list, one over the reader. Empty areas drag the window; the hairline under
 * it and the one between the columns run the full height.
 */
export default function Toolbar({ list, reader, insetForTrafficLights }: { list: ReactNode; reader: ReactNode; insetForTrafficLights: boolean }) {
  return (
    <header className="grid h-toolbar flex-none grid-cols-panes bg-bg-toolbar shadow-hairline-b select-none">
      <div
        data-tauri-drag-region
        className={clsx("flex min-w-0 items-center gap-1 pr-2.5 shadow-hairline-r", insetForTrafficLights ? "pl-traffic-zone" : "pl-5")}
      >
        {list}
      </div>
      <div data-tauri-drag-region className="flex min-w-0 items-center gap-1 pl-6 pr-2.5">
        {reader}
      </div>
    </header>
  );
}

/** The scope title and its subtitle ("122 sessions"), over the list. */
export function ScopeTitle({ title, subtitle }: { title: string; subtitle: string }) {
  return (
    <div data-tauri-drag-region className="flex min-w-0 flex-col">
      <h1 data-tauri-drag-region className="truncate text-headline">
        {title}
      </h1>
      <p data-tauri-drag-region className="truncate text-subheadline tabular-nums text-label-2">
        {subtitle}
      </p>
    </div>
  );
}

/**
 * The scope title as a menu, for when the sidebar is hidden (spec 7.2, 8): the same
 * sources and counts as the sidebar, so every scope stays one click away.
 */
export function ScopeMenu({
  title,
  subtitle,
  sections,
  selected,
  onChoose,
  onNewFolder,
}: {
  title: string;
  subtitle: string;
  sections: SourceSection[];
  selected: Source;
  onChoose: (source: Source) => void;
  onNewFolder: () => void;
}) {
  const keyOf = (source: Source) => (source.kind === "all" ? "all" : `${source.kind}:${source.id}`);
  const rows = sections.flatMap((s) => s.rows);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={`${title}, choose what to browse`}
          className="-ml-1.5 flex min-w-0 flex-col rounded-control px-1.5 text-left outline-none hover:bg-fill-2 focus-visible:shadow-focus"
        >
          <span className="flex min-w-0 max-w-full items-center gap-0.5">
            <span className="truncate text-headline">{title}</span>
            <ChevronDown className={clsx(ICON.small, "text-label-2")} aria-hidden="true" />
          </span>
          <span className="max-w-full truncate text-subheadline tabular-nums text-label-2">{subtitle}</span>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent>
        <DropdownMenuRadioGroup
          value={keyOf(selected)}
          onValueChange={(key) => {
            const row = rows.find((r) => keyOf(r.source) === key);
            if (row) onChoose(row.source);
          }}
        >
          {sections.map((section, i) => (
            <div key={section.title ?? "library"} role="group" aria-label={section.title ?? "Library"}>
              {i > 0 && <DropdownMenuSeparator />}
              {section.title && <DropdownMenuLabel>{section.title}</DropdownMenuLabel>}
              {section.rows.map((row) => (
                <DropdownMenuRadioItem
                  key={row.key}
                  value={keyOf(row.source)}
                  className={clsx(row.missing && "text-label-3")}
                  aria-label={row.note ? `${row.label}, ${row.note}` : undefined}
                >
                  <span className="truncate">{row.label}</span>
                  {row.count !== null && <MenuTrailing>{row.count}</MenuTrailing>}
                </DropdownMenuRadioItem>
              ))}
            </div>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={onNewFolder}>New Folder…</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
