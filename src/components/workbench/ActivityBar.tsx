import type { ReactNode } from "react";

import { AgentsIcon, FilesIcon } from "./icons";

export type SidebarView = "explorer" | "agents";

const VIEWS: { id: SidebarView; label: string; icon: ReactNode }[] = [
  { id: "explorer", label: "Explorer", icon: <FilesIcon /> },
  { id: "agents", label: "Agents", icon: <AgentsIcon /> },
];

type Props = {
  view: SidebarView | null;
  runningCount: number;
  onSelect: (view: SidebarView) => void;
};

/** Far-left icon strip. Clicking the active view collapses the sidebar. */
export default function ActivityBar({ view, runningCount, onSelect }: Props) {
  return (
    <nav className="activitybar" aria-label="Views">
      {VIEWS.map((v) => (
        <button
          key={v.id}
          type="button"
          className={`activitybar__btn${view === v.id ? " activitybar__btn--active" : ""}`}
          aria-label={v.label}
          aria-pressed={view === v.id}
          title={v.label}
          onClick={() => onSelect(v.id)}
        >
          {v.icon}
          {v.id === "agents" && runningCount > 0 && (
            <span className="activitybar__badge" aria-hidden="true">
              {runningCount}
            </span>
          )}
        </button>
      ))}
    </nav>
  );
}
