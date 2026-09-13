import type { KeyboardEvent } from "react";

import { formatRelative, formatTime } from "../../format";
import type { TaskDto } from "../../ipc";
import ActivityList from "./ActivityList";
import { CloseIcon, FileIcon } from "./icons";
import { useActivity, useDecisions } from "./useTasks";

export type PanelTab = "output" | "changes" | "history";

const TABS: { id: PanelTab; label: string }[] = [
  { id: "output", label: "Agent Output" },
  { id: "changes", label: "Changes" },
  { id: "history", label: "History" },
];

type Props = {
  height: number;
  tab: PanelTab;
  onTabChange: (tab: PanelTab) => void;
  onClose: () => void;
  task: TaskDto | undefined;
  onOpenChange: (task: TaskDto, relPath: string) => void;
  /** Repository whose shared decision log the History tab shows. */
  repoPath: string | null;
};

/** Bottom panel: the selected agent's output, its changed files, and the decision log. */
export default function BottomPanel({
  height,
  tab,
  onTabChange,
  onClose,
  task,
  onOpenChange,
  repoPath,
}: Props) {
  const activity = useActivity(tab === "output" && task ? task.id : null, task?.state === "running");
  const decisions = useDecisions(repoPath, tab === "history");

  function onKeyDown(event: KeyboardEvent) {
    if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
    event.preventDefault();
    const step = event.key === "ArrowRight" ? 1 : TABS.length - 1;
    const next = TABS[(TABS.findIndex((t) => t.id === tab) + step) % TABS.length].id;
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
                <span className="panel__count">{task.changed_files_total ?? task.changed_files.length}</span>
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
        {tab === "history" ? (
          <DecisionLog items={decisions.items} error={decisions.error} />
        ) : !task ? (
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
            {(task.changed_files_total ?? 0) > task.changed_files.length && (
              <li className="wb-note">
                …and {(task.changed_files_total ?? 0) - task.changed_files.length} more (list capped)
              </li>
            )}
          </ul>
        )}
      </div>
    </section>
  );
}

/** The repository's shared decision log, newest first. */
function DecisionLog({
  items,
  error,
}: {
  items: ReturnType<typeof useDecisions>["items"];
  error: string | null;
}) {
  if (error) {
    return (
      <p className="wb-note wb-note--error wb-view-pad" role="alert">
        {error}
      </p>
    );
  }
  if (items === null) {
    return (
      <p className="wb-note wb-view-pad" role="status">
        Loading history…
      </p>
    );
  }
  if (items.length === 0) return <p className="wb-note wb-view-pad">No decisions recorded yet.</p>;
  return (
    <ul className="decisions" aria-label="Decision log">
      {items.map((decision, i) => (
        <li key={`${decision.at_ms}-${decision.task_id}-${i}`} className="decisions__row">
          <time className="decisions__time" title={formatTime(decision.at_ms)}>
            {formatRelative(decision.at_ms)}
          </time>
          <span className={`decisions__kind decisions__kind--${decision.kind}`}>
            {decision.kind.replace(/_/g, " ")}
          </span>
          <span className="decisions__title">{decision.task_title}</span>
          <span className="decisions__detail">{decision.detail}</span>
        </li>
      ))}
    </ul>
  );
}
