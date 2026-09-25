import { memo, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

import { formatRelative, formatTime } from "../../format";
import type { TaskDto } from "../../ipc";
import { AgentChip, StateBadge } from "./badges";
import { needsUser } from "./board";
import { DiffIcon, OverviewIcon, PlusIcon, RevealIcon, StopIcon, TrashIcon, WarningIcon } from "./icons";
import { SidebarHeader } from "./Sidebar";
import { errorText, shortcut } from "./state";

export type TaskActions = {
  onSelect: (id: string) => void;
  onStop: (id: string) => Promise<void>;
  onDiscard: (id: string) => Promise<void>;
  onReveal: (id: string) => Promise<void>;
  onOpenDiff: (id: string) => void;
};

type Props = TaskActions & {
  workspace: string | null;
  tasks: TaskDto[] | null;
  listError: string | null;
  loadWarning: string | null;
  selectedTaskId: string | null;
  /** The overview is on stage (no agent or file open). */
  overviewActive: boolean;
  onShowOverview: () => void;
  onNewAgent: () => void;
  /** Show tasks from every repository, not only the open workspace. */
  allRepos: boolean;
  onAllReposChange: (all: boolean) => void;
  /** Tasks hidden by the repository scope, for the toggle's label. */
  hiddenCount: number;
  /** The Agents / Files switch, shown in place of the title. */
  switcher?: ReactNode;
};

/** The Agents sidebar view: the overview entry and every task, live. */
export default function AgentsPanel(props: Props) {
  const { tasks, listError, loadWarning, selectedTaskId, hiddenCount } = props;
  // With no folder open there is nothing to filter by, so every repository shows.
  const allRepos = props.allRepos || !props.workspace;
  const listRef = useRef<HTMLUListElement>(null);
  const [focusId, setFocusId] = useState<string | null>(null);
  const attention = (tasks ?? []).filter(needsUser).length;
  const running = (tasks ?? []).filter((t) => t.state === "running").length;

  // One tab stop for the list (roving tabindex): the focused, selected, or first row.
  const tabStop =
    (tasks ?? []).find((t) => t.id === focusId)?.id ??
    (tasks ?? []).find((t) => t.id === selectedTaskId)?.id ??
    tasks?.[0]?.id;

  function onListKeyDown(event: KeyboardEvent<HTMLUListElement>) {
    const rows = Array.from(listRef.current?.querySelectorAll<HTMLButtonElement>(".agent-row__main") ?? []);
    const index = rows.indexOf(document.activeElement as HTMLButtonElement);
    if (index < 0) return;
    let next: HTMLButtonElement | undefined;
    if (event.key === "ArrowDown") next = rows[Math.min(index + 1, rows.length - 1)];
    else if (event.key === "ArrowUp") next = rows[Math.max(index - 1, 0)];
    else if (event.key === "Home") next = rows[0];
    else if (event.key === "End") next = rows[rows.length - 1];
    else return;
    event.preventDefault();
    next?.focus();
  }

  return (
    <section className="wb-view wb-view--agents" aria-label="Agents">
      <SidebarHeader title="Agents" switcher={props.switcher}>
        <button
          type="button"
          className={`scope-toggle${allRepos ? " scope-toggle--on" : ""}`}
          aria-pressed={allRepos}
          disabled={!props.workspace}
          title={
            !props.workspace
              ? "Open a folder to show only its tasks"
              : allRepos
                ? "Showing tasks from every repository"
                : "Showing only tasks in the open repository"
          }
          onClick={() => props.onAllReposChange(!props.allRepos)}
        >
          All repositories
          {!allRepos && hiddenCount > 0 && <span className="scope-toggle__count">{hiddenCount}</span>}
        </button>
        <button
          type="button"
          className="wb-icon-btn"
          aria-label="New agent"
          title={`New agent (${shortcut("N")})`}
          disabled={!props.workspace}
          onClick={props.onNewAgent}
        >
          <PlusIcon />
        </button>
      </SidebarHeader>
      <div className="wb-view__body">
        {loadWarning && (
          <p className="wb-note wb-note--error" role="alert">
            {loadWarning}
          </p>
        )}
        <button
          type="button"
          className={`overview-row${props.overviewActive ? " overview-row--on" : ""}`}
          aria-current={props.overviewActive || undefined}
          onClick={props.onShowOverview}
        >
          <OverviewIcon />
          <span className="overview-row__label">Overview</span>
          {attention > 0 && (
            <span className="overview-row__count overview-row__count--attention" title="Need you">
              {attention}
            </span>
          )}
          {running > 0 && (
            <span className="overview-row__count" title="Running">
              {running}
            </span>
          )}
        </button>
        <div className="agents-pane">
          {listError && (
            <p className="wb-note wb-note--error" role="alert">
              {listError}
            </p>
          )}
          {tasks === null ? (
            <p className="wb-note" role="status">
              Loading tasks…
            </p>
          ) : tasks.length === 0 ? (
            <p className="wb-note">
              {!allRepos && hiddenCount > 0
                ? `No agents in this repository (${hiddenCount} in others).`
                : "No agents yet."}
            </p>
          ) : (
            <ul className="agents" aria-label="Tasks" ref={listRef} onKeyDown={onListKeyDown}>
              {tasks.map((task, i) => (
                <TaskRow
                  key={task.id}
                  task={task}
                  index={i}
                  selected={task.id === selectedTaskId}
                  tabStop={task.id === tabStop}
                  onFocusRow={setFocusId}
                  onSelect={props.onSelect}
                  onStop={props.onStop}
                  onDiscard={props.onDiscard}
                  onReveal={props.onReveal}
                  onOpenDiff={props.onOpenDiff}
                />
              ))}
            </ul>
          )}
        </div>
      </div>
    </section>
  );
}

type RowProps = TaskActions & {
  task: TaskDto;
  index: number;
  selected: boolean;
  tabStop: boolean;
  onFocusRow: (id: string) => void;
};

const TaskRow = memo(function TaskRow({
  task,
  index,
  selected,
  tabStop,
  onFocusRow,
  onSelect,
  onStop,
  onDiscard,
  onReveal,
  onOpenDiff,
}: RowProps) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  async function act(action: (id: string) => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await action(task.id);
    } catch (e) {
      if (mountedRef.current) setError(errorText(e));
    } finally {
      if (mountedRef.current) setBusy(false);
    }
  }

  const fileCount = task.changed_files_total ?? task.changed_files.length;
  const uncommitted = task.uncommitted_count ?? 0;
  const overlapCount = task.overlaps?.length ?? 0;
  const merged = Boolean(task.merged_into);
  const hint = [
    task.branch,
    `${task.commits_ahead} ${task.commits_ahead === 1 ? "commit" : "commits"} ahead · ${fileCount} ${fileCount === 1 ? "file" : "files"} changed`,
    uncommitted > 0 && !merged ? `${uncommitted} uncommitted ${uncommitted === 1 ? "change" : "changes"}` : null,
    overlapCount > 0
      ? `Overlaps ${overlapCount} ${overlapCount === 1 ? "task" : "tasks"}: ${task.overlaps!.map((o) => o.title).join(", ")}`
      : null,
    task.merged_into ? `Merged into ${task.merged_into}` : null,
    task.auto_handoff === false && !merged ? "manual handoff" : null,
    index < 9 ? `${shortcut(String(index + 1))} to open` : null,
  ]
    .filter(Boolean)
    .join(" · ");
  const stats = !merged && fileCount > 0 ? `${fileCount} ${fileCount === 1 ? "file" : "files"}` : null;
  const preview = merged ? null : (task.attention ?? task.last_activity);

  return (
    <li
      className={`agent-row${selected ? " agent-row--selected" : ""}${task.attention ? " agent-row--attention" : ""}`}
      aria-label={task.title}
    >
      <button
        type="button"
        className="agent-row__main"
        title={hint}
        aria-label={task.title}
        aria-current={selected || undefined}
        tabIndex={tabStop ? 0 : -1}
        onFocus={() => onFocusRow(task.id)}
        onClick={() => onSelect(task.id)}
      >
        <span className="agent-row__head">
          <span className="agent-row__title">{task.title}</span>
          {task.attention && (
            <span className="row-flag row-flag--attention" title={task.attention} aria-label="Needs attention">
              <WarningIcon />
            </span>
          )}
          <time className="agent-row__time" title={formatTime(task.created_at_ms)}>
            {formatRelative(task.created_at_ms)}
          </time>
        </span>
        <span className="agent-row__meta">
          <StateBadge task={task} compact />
          <AgentChip agent={task.agent} />
          {stats && <span className="agent-row__stats">{stats}</span>}
        </span>
        {preview && (
          <span className={`agent-row__preview${task.attention ? " agent-row__preview--attention" : ""}`}>
            {preview}
          </span>
        )}
      </button>
      {!confirming && (
        // Pointer shortcuts; the keyboard reaches the same actions in the agent view and ⌘K.
        <div className="agent-row__actions">
          {task.state === "running" && (
            <button
              type="button"
              className="wb-icon-btn"
              tabIndex={-1}
              aria-label="Stop"
              title="Stop"
              disabled={busy}
              onClick={() => void act(onStop)}
            >
              <StopIcon />
            </button>
          )}
          <button
            type="button"
            className="wb-icon-btn"
            tabIndex={-1}
            aria-label="Open diff"
            title="Open diff"
            onClick={() => onOpenDiff(task.id)}
          >
            <DiffIcon />
          </button>
          <button
            type="button"
            className="wb-icon-btn"
            tabIndex={-1}
            aria-label="Reveal in Finder"
            title="Reveal in Finder"
            onClick={() => void act(onReveal)}
          >
            <RevealIcon />
          </button>
          <button
            type="button"
            className="wb-icon-btn wb-icon-btn--danger"
            tabIndex={-1}
            aria-label="Discard"
            title="Discard"
            disabled={busy}
            onClick={() => setConfirming(true)}
          >
            <TrashIcon />
          </button>
        </div>
      )}
      {error && (
        <p className="wb-note wb-note--error" role="alert">
          {error}
        </p>
      )}
      {confirming && (
        <div className="agent-row__confirm">
          <span>Delete this worktree and branch?</span>
          <button
            type="button"
            className="wb-btn wb-btn--danger"
            disabled={busy}
            onClick={() =>
              void act(onDiscard).then(() => {
                if (mountedRef.current) setConfirming(false);
              })
            }
          >
            Confirm discard
          </button>
          <button type="button" className="wb-btn" onClick={() => setConfirming(false)}>
            Cancel
          </button>
        </div>
      )}
    </li>
  );
});
