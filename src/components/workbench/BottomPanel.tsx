import type { KeyboardEvent } from "react";

import type { TaskDto } from "../../ipc";
import ActivityList from "./ActivityList";
import { CloseIcon, FileIcon } from "./icons";
import { useActivity } from "./useTasks";

export type PanelTab = "output" | "changes";

const TABS: { id: PanelTab; label: string }[] = [
  { id: "output", label: "Agent Output" },
  { id: "changes", label: "Changes" },
];

type Props = {
  height: number;
  tab: PanelTab;
  onTabChange: (tab: PanelTab) => void;
  onClose: () => void;
  task: TaskDto | undefined;
  onOpenChange: (task: TaskDto, relPath: string) => void;
};

/** Bottom panel: the selected agent's output and its changed files. */
export default function BottomPanel({ height, tab, onTabChange, onClose, task, onOpenChange }: Props) {
  const activity = useActivity(tab === "output" && task ? task.id : null, task?.state === "running");

  function onKeyDown(event: KeyboardEvent) {
    if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
    event.preventDefault();
    const next = tab === "output" ? "changes" : "output";
    onTabChange(next);
    document.getElementById(`panel-tab-${next}`)?.focus();
  }

  return (
    <section className="panel" style={{ height }} aria-label="Panel">
      <div className="panel__bar">
        <div className="panel__tabs" role="tablist" aria-label="Panel views" onKeyDown={onKeyDown}>
          {TABS.map((t) => (
            <button
              key={t.id}
              id={`panel-tab-${t.id}`}
              type="button"
              role="tab"
              aria-selected={tab === t.id}
              aria-controls={`panel-view-${t.id}`}
              tabIndex={tab === t.id ? 0 : -1}
              className={`panel__tab${tab === t.id ? " panel__tab--active" : ""}`}
              onClick={() => onTabChange(t.id)}
            >
              {t.label}
              {t.id === "changes" && task && task.changed_files.length > 0 && (
                <span className="panel__count">{task.changed_files.length}</span>
              )}
            </button>
          ))}
        </div>
        {task && <span className="panel__context">{task.title}</span>}
        <button
          type="button"
          className="wb-icon-btn"
          aria-label="Close panel"
          title="Close panel (⌘J)"
          onClick={onClose}
        >
          <CloseIcon />
        </button>
      </div>
      <div
        id={`panel-view-${tab}`}
        role="tabpanel"
        aria-labelledby={`panel-tab-${tab}`}
        className="panel__body"
      >
        {!task ? (
          <p className="wb-note wb-view-pad">Select an agent to see its output.</p>
        ) : tab === "output" ? (
          <ActivityList
            compact
            items={activity.items}
            error={activity.error}
            label={`Output of ${task.title}`}
          />
        ) : task.changed_files.length === 0 ? (
          <p className="wb-note wb-view-pad">No changed files.</p>
        ) : (
          <ul className="changes" aria-label={`Changed files in ${task.title}`}>
            {task.changed_files.map((file) => (
              <li key={file}>
                <button type="button" className="changes__row" onClick={() => onOpenChange(task, file)}>
                  <FileIcon />
                  <span className="changes__name">{file.split("/").pop()}</span>
                  <span className="changes__dir mono">{file.includes("/") ? file.slice(0, file.lastIndexOf("/")) : ""}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
