import { clsx } from "clsx";
import type { LucideIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { ICON } from "./ui/button";
import {
  Command as Cmdk,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "./ui/command";

/** Palette sections in display order (spec 9.8). View holds what Mail keeps in its View menu. */
export const GROUPS = ["Actions", "View", "Sessions", "Repositories", "Folders"] as const;
export type PaletteGroup = (typeof GROUPS)[number];

export interface Command {
  id: string;
  group: PaletteGroup;
  label: string;
  icon: LucideIcon;
  /** A shortcut in Apple glyph order (⇧⌘C). */
  shortcut?: string;
  /** Trailing facts when there is no shortcut: a date, a session count. */
  detail?: string;
  run: () => void;
}

/** With an empty query, the long groups show only their first rows; typing reaches the rest. */
const IDLE_LIMIT = 5;
const QUERY_LIMIT = 20;
const ARCHIVE_DEBOUNCE_MS = 160;
const ARCHIVE_MIN_LENGTH = 2;
const LONG_GROUPS: ReadonlySet<PaletteGroup> = new Set(["Sessions", "Repositories", "Folders"]);

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
  const detail = command.detail ? fuzzyScore(query, command.detail) : null;
  if (label === null && detail === null) return null;
  return Math.max(label ?? -Infinity, detail === null ? -Infinity : detail - 30);
}

/** The commands to show, grouped in GROUPS order and ranked by score within a group. */
export function rankCommands(query: string, commands: Command[], archive: Command[]): [PaletteGroup, Command[]][] {
  const trimmed = query.trim();
  const best = new Map<string, { command: Command; score: number; index: number }>();
  [...commands, ...archive].forEach((command, index) => {
    const local = scoreCommand(trimmed, command);
    // A full-text hit can match a session's body when its title does not, so it ranks last instead of vanishing.
    const score = local ?? (index >= commands.length ? -100 - index : null);
    if (score === null) return;
    const seen = best.get(command.id);
    if (!seen || score > seen.score) best.set(command.id, { command, score, index });
  });
  const ranked = [...best.values()].sort((a, b) => b.score - a.score || a.index - b.index);
  const limit = trimmed === "" ? IDLE_LIMIT : QUERY_LIMIT;
  return GROUPS.map((group): [PaletteGroup, Command[]] => {
    const rows = ranked.filter((m) => m.command.group === group).map((m) => m.command);
    return [group, LONG_GROUPS.has(group) ? rows.slice(0, limit) : rows];
  }).filter(([, rows]) => rows.length > 0);
}

type ArchiveState =
  | { status: "idle" }
  | { status: "loading"; query: string }
  | { status: "ready"; query: string; items: Command[] }
  | { status: "failed"; query: string };

/**
 * The command palette (spec 9.8), Spotlight-like: every action with its shortcut, the
 * view options, and navigation to sessions, repositories and folders, plus a
 * full-text search of the archive for sessions that are not loaded.
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
  const [archive, setArchive] = useState<ArchiveState>({ status: "idle" });
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

  const groups = useMemo(
    () => rankCommands(trimmed, commands, archive.status === "ready" && archive.query === trimmed ? archive.items : []),
    [archive, commands, trimmed],
  );

  function run(command: Command) {
    onClose();
    command.run();
  }

  const status =
    archive.status === "loading" ? "Searching the archive…" : archive.status === "failed" ? "The archive search failed. Local matches are still shown." : "";

  return (
    <CommandDialog open onOpenChange={(open) => !open && onClose()} label="Command Palette">
      <Cmdk shouldFilter={false} loop label="Search sessions and actions">
        <CommandInput
          aria-label="Search sessions and actions"
          placeholder="Search sessions and actions"
          maxLength={256}
          value={query}
          onValueChange={setQuery}
        />
        <CommandList>
          {archive.status !== "loading" && <CommandEmpty>No Results for “{trimmed}”</CommandEmpty>}
          {groups.map(([group, rows]) => (
            <CommandGroup key={group} heading={group}>
              {rows.map((command) => {
                const Icon = command.icon;
                const trailing = command.shortcut ?? command.detail;
                return (
                  <CommandItem key={command.id} value={command.id} onSelect={() => run(command)}>
                    <Icon className={clsx(ICON.body, "text-label-2 group-aria-selected:text-on-selected")} aria-hidden="true" />
                    <span className="min-w-0 flex-1 truncate">{command.label}</span>
                    {trailing && <CommandShortcut>{trailing}</CommandShortcut>}
                  </CommandItem>
                );
              })}
            </CommandGroup>
          ))}
        </CommandList>
        <p role="status" aria-live="polite" className={clsx("px-4 text-callout text-label-2", status && "pb-2.5")}>
          {status}
        </p>
      </Cmdk>
    </CommandDialog>
  );
}
