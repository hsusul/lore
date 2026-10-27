import { Folder, FolderGit2, FolderX, Inbox, SquareTerminal, type LucideIcon } from "lucide-react";

import type { DetectedAgent, FolderSummary, RepositorySummary } from "../ipc";
import { sameSource, type Source } from "../hooks/workspace";

/** One row of the sidebar and of the toolbar scope menu, which list the same sources. */
export interface SourceRow {
  key: string;
  source: Source;
  label: string;
  icon: LucideIcon;
  count: number | null;
  /** Why the row is dimmed or flagged, said in words for assistive tech and on hover. */
  note: string | null;
  missing: boolean;
  folder: FolderSummary | null;
}

export interface SourceSection {
  /** `null` for the leading All Sessions row, which has no header. */
  title: string | null;
  rows: SourceRow[];
}

export function sourceSections({
  agents,
  repositories,
  folders,
}: {
  agents: DetectedAgent[];
  repositories: RepositorySummary[];
  folders: FolderSummary[];
}): SourceSection[] {
  const known = agents.filter((a) => a.session_count > 0 || a.installed);
  const total = known.length > 0 ? agents.reduce((sum, a) => sum + a.session_count, 0) : null;
  const row = (r: Omit<SourceRow, "note" | "missing" | "folder"> & Partial<SourceRow>): SourceRow => ({
    note: null,
    missing: false,
    folder: null,
    ...r,
  });
  const sections: SourceSection[] = [
    { title: null, rows: [row({ key: "all", source: { kind: "all" }, label: "All Sessions", icon: Inbox, count: total })] },
    {
      title: "Repositories",
      rows: repositories.map((r) =>
        row({
          key: `repo-${r.id}`,
          source: { kind: "repository", id: r.id },
          label: r.display_name,
          icon: r.is_missing ? FolderX : FolderGit2,
          count: r.session_count,
          missing: r.is_missing,
          note: r.is_missing ? "Missing" : r.identity_confidence === "low" ? "Low confidence" : null,
        }),
      ),
    },
    {
      title: "Folders",
      rows: folders.map((f) =>
        row({ key: `folder-${f.id}`, source: { kind: "folder", id: f.id }, label: f.name, icon: Folder, count: f.session_count, folder: f }),
      ),
    },
    {
      title: "Agents",
      rows: known.map((a) =>
        row({ key: `agent-${a.id}`, source: { kind: "agent", id: a.id }, label: a.display_name, icon: SquareTerminal, count: a.session_count }),
      ),
    },
  ];
  return sections.filter((s) => s.rows.length > 0);
}

export function sourceRow(sections: SourceSection[], source: Source): SourceRow | null {
  for (const section of sections) {
    const found = section.rows.find((r) => sameSource(r.source, source));
    if (found) return found;
  }
  return null;
}
