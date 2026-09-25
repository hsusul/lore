import { useId, useState } from "react";

import { formatRelative, formatTime } from "../../format";
import type { TaskDto } from "../../ipc";
import { AgentChip, StatusIcon, taskStatus } from "./badges";
import { groupTasks, type BoardGroup } from "./board";
import { Mark, MergeIcon, PlusIcon } from "./icons";
import { baseName, shortcut } from "./state";

type Filter = "all" | BoardGroup["id"];

const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "attention", label: "Needs you" },
  { id: "running", label: "Running" },
  { id: "ready", label: "Ready to merge" },
  { id: "idle", label: "To review" },
  { id: "merged", label: "Merged" },
];

type Props = {
  workspace: string | null;
  /** The tasks in scope (the open repository, or every repository). */
  tasks: TaskDto[] | null;
  onSelect: (id: string) => void;
  onNewAgent: () => void;
  onOpenFolder: () => void;
  onOpenMergeQueue: () => void;
};

/**
 * The overview stage, shown when no agent or file is open: where every agent
 * stands, what needs you first. Starting an agent is the New agent dialog's job.
 */
export default function Home({
  workspace,
  tasks,
  onSelect,
  onNewAgent,
  onOpenFolder,
  onOpenMergeQueue,
}: Props) {
  const [filter, setFilter] = useState<Filter>("all");
  const groups = groupTasks(tasks ?? []);
  const inGroup = (id: BoardGroup["id"]) => groups.find((g) => g.id === id)?.tasks ?? [];
  const shown = filter === "all" ? groups.flatMap((g) => g.tasks) : inGroup(filter);
  // Empty filters stay out of the way, except the one in use.
  const filters = FILTERS.filter((f) => f.id === "all" || f.id === filter || inGroup(f.id as BoardGroup["id"]).length > 0);
  const branch = (tasks ?? []).find((t) => t.repo_path === workspace && t.repo_branch)?.repo_branch;
  const listId = useId();
  const count = (id: BoardGroup["id"]) => inGroup(id).length;
  const summary = [
    branch ?? null,
    count("running") > 0 ? `${count("running")} running` : null,
    count("attention") > 0 ? `${count("attention")} need${count("attention") === 1 ? "s" : ""} you` : null,
    count("ready") > 0 ? `${count("ready")} ready to merge` : null,
  ].filter((part): part is string => part !== null);

  return (
    <div className="home" role="region" aria-label="Overview">
      <div className="home__inner">
        {workspace ? (
          <header className="home__head">
            <div className="home__heading">
              <h2 className="home__name">{baseName(workspace)}</h2>
              <p className="home__summary">
                {summary.length > 0 ? summary.map((part) => <span key={part}>{part}</span>) : <span>No agents yet</span>}
              </p>
            </div>
            <button type="button" className="wb-btn wb-btn--primary" onClick={onNewAgent}>
              <PlusIcon />
              New agent
              <kbd>{shortcut("N")}</kbd>
            </button>
          </header>
        ) : (
          <header className="home__header home__header--welcome">
            <Mark />
            <h2 className="home__title">Lore</h2>
            <p className="home__lede">
              Run coding agents in parallel, each in its own git worktree and branch. Your checkout is not touched.
            </p>
            <div className="home__actions">
              <button type="button" className="wb-btn wb-btn--primary" onClick={onOpenFolder}>
                Open folder…
              </button>
              <span className="home__keys">
                <kbd>{shortcut("K")}</kbd> Search <kbd>{shortcut("N")}</kbd> New agent
              </span>
            </div>
          </header>
        )}

        {tasks === null ? (
          <p className="wb-note" role="status">
            Loading agents…
          </p>
        ) : groups.length === 0 ? (
          workspace && (
            <div className="home__empty">
              <p className="home__empty-title">No agents yet</p>
              <p>Each agent gets its own worktree and branch, so your checkout stays untouched until you merge.</p>
              <button type="button" className="wb-btn" onClick={onNewAgent}>
                <PlusIcon />
                New agent
              </button>
            </div>
          )
        ) : (
          <section className="tasks" aria-label="Agents">
            <div className="tasks__bar">
              <div className="tasks__filters" role="tablist" aria-label="Filter agents">
                {filters.map((f) => {
                  const n = f.id === "all" ? (tasks ?? []).length : inGroup(f.id as BoardGroup["id"]).length;
                  return (
                    <button
                      key={f.id}
                      type="button"
                      role="tab"
                      aria-selected={filter === f.id}
                      aria-controls={listId}
                      className={`tasks__filter${filter === f.id ? " tasks__filter--on" : ""}`}
                      onClick={() => setFilter(f.id)}
                    >
                      {f.label}
                      <span className={`tasks__count${f.id === "attention" ? " tasks__count--attention" : ""}`}>
                        {n}
                      </span>
                    </button>
                  );
                })}
              </div>
              {inGroup("ready").length > 0 && (
                <button type="button" className="wb-btn wb-btn--small wb-btn--ghost" onClick={onOpenMergeQueue}>
                  <MergeIcon /> Merge queue
                </button>
              )}
            </div>
            <ul
              className="tasks__list"
              id={listId}
              role="tabpanel"
              aria-label={FILTERS.find((f) => f.id === filter)!.label}
            >
              {shown.map((task) => (
                <li key={task.id}>
                  <TaskRow task={task} onSelect={onSelect} />
                </li>
              ))}
              {shown.length === 0 && <li className="tasks__none">Nothing here.</li>}
            </ul>
          </section>
        )}
      </div>
    </div>
  );
}

function TaskRow({ task, onSelect }: { task: TaskDto; onSelect: (id: string) => void }) {
  const status = taskStatus(task);
  const files = task.changed_files_total ?? task.changed_files.length;
  const detail = task.merged_into
    ? `Merged into ${task.merged_into}`
    : (task.attention ??
      ((task.claim_conflicts?.length ?? 0) > 0 ? "Changed files another agent owns" : task.last_activity));
  const id = useId();
  return (
    // Named by its title; what it is doing and where it stands describe it.
    <button
      type="button"
      className={`task-row task-row--${status}`}
      aria-label={task.title}
      aria-describedby={`${id}-detail ${id}-meta`}
      onClick={() => onSelect(task.id)}
    >
      <StatusIcon status={status} />
      <span className="task-row__title">{task.title}</span>
      <span className="task-row__detail" id={`${id}-detail`}>
        {detail}
      </span>
      <span className="task-row__meta" id={`${id}-meta`}>
        <AgentChip agent={task.agent} />
        {files > 0 && <span>{`${files} ${files === 1 ? "file" : "files"}`}</span>}
      </span>
      <time className="task-row__time" title={formatTime(task.created_at_ms)}>
        {formatRelative(task.created_at_ms)}
      </time>
    </button>
  );
}
