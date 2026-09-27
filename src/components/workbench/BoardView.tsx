import { useEffect, useId, useRef, useState, type ReactNode } from "react";

import { formatRelative, formatTime } from "../../format";
import type { ContinueTaskRequest, TaskAgent, TaskDto } from "../../ipc";
import { AGENT_SHORT, USAGE_LIMIT, usageLimitHandoffPrompt } from "./AgentView";
import { StatusIcon, taskStatus, type Status } from "./badges";
import { groupTasks, readyToMerge, type BoardGroup } from "./board";
import { MergeIcon, WarningIcon } from "./icons";
import NewAgentForm from "./NewAgentForm";
import { AGENT_LABELS, baseName, errorText, stateLabel, type HomeView } from "./state";

type Props = {
  workspace: string;
  /** The tasks in scope (the open repository, or every repository). */
  tasks: TaskDto[] | null;
  onSelect: (id: string) => void;
  onOpenDiff: (id: string) => void;
  onContinue: (request: ContinueTaskRequest) => Promise<void>;
  onOpenMergeQueue: () => void;
  onOpenFolder: () => void;
  /** An agent launched from the board's composer; the board stays on screen. */
  onCreated: (task: TaskDto) => void;
  /** Whether a merge queue owns the repository's merge slot, which pauses handoffs. */
  queueRunning: (repoPath: string) => boolean;
  /** The Board / List switch. */
  switcher?: ReactNode;
};

type Column = { id: BoardGroup["id"]; title: string; status?: Status; empty?: string };

/** Columns with an empty message always show, so the layout holds still; the rest appear when they have work. */
const COLUMNS: Column[] = [
  { id: "attention", title: "Needs you", status: "attention", empty: "Nothing needs you." },
  { id: "running", title: "Running", status: "running", empty: "No agents running." },
  { id: "ready", title: "Ready to merge", status: "finished", empty: "Nothing to merge yet." },
  { id: "idle", title: "To review" },
  { id: "merged", title: "Merged", status: "merged" },
];

/**
 * The Ember overview: every agent in a column by what it needs from you, with
 * the action it most likely needs on its card, under a composer that can
 * launch one agent or several.
 */
export default function BoardView({
  workspace,
  tasks,
  onSelect,
  onOpenDiff,
  onContinue,
  onOpenMergeQueue,
  onOpenFolder,
  onCreated,
  queueRunning,
  switcher,
}: Props) {
  const groups = groupTasks(tasks ?? []);
  const inGroup = (id: BoardGroup["id"]) => groups.find((g) => g.id === id)?.tasks ?? [];
  const columns = COLUMNS.filter((c) => c.empty || inGroup(c.id).length > 0);
  const branch = (tasks ?? []).find((t) => t.repo_path === workspace && t.repo_branch)?.repo_branch;
  const count = (id: BoardGroup["id"]) => inGroup(id).length;
  const summary = [
    count("attention") > 0 ? `${count("attention")} need${count("attention") === 1 ? "s" : ""} you` : null,
    count("running") > 0 ? `${count("running")} running` : null,
    count("ready") > 0 ? `${count("ready")} ready to merge` : null,
  ].filter((part): part is string => part !== null);

  return (
    <div className="board" role="region" aria-label="Overview">
      <header className="board__head">
        <div className="board__heading">
          <h2 className="board__name">
            {baseName(workspace)}
            {branch && <span className="board__branch"> on {branch}</span>}
          </h2>
          <p className="board__summary">{summary.length > 0 ? summary.join(" · ") : "No agents yet"}</p>
        </div>
        {switcher}
      </header>
      <div className="board__composer">
        <NewAgentForm workspace={workspace} onCreated={onCreated} onOpenFolder={onOpenFolder} allowSplit />
      </div>
      {tasks === null ? (
        <p className="wb-note" role="status">
          Loading agents…
        </p>
      ) : (
        <div
          className="board__cols"
          style={{
            // Columns keep a readable width; a narrow window scrolls them sideways.
            gridTemplateColumns: columns
              .map((c) => (c.id === "merged" ? "minmax(140px, 0.72fr)" : "minmax(190px, 1fr)"))
              .join(" "),
          }}
        >
          {columns.map((column) => (
            <BoardColumn
              key={column.id}
              column={column}
              tasks={inGroup(column.id)}
              onSelect={onSelect}
              onOpenDiff={onOpenDiff}
              onContinue={onContinue}
              onOpenMergeQueue={onOpenMergeQueue}
              queueRunning={queueRunning}
            />
          ))}
        </div>
      )}
    </div>
  );
}

type CardActions = Pick<Props, "onSelect" | "onOpenDiff" | "onContinue" | "queueRunning">;

function BoardColumn({
  column,
  tasks,
  onOpenMergeQueue,
  ...actions
}: CardActions & { column: Column; tasks: TaskDto[]; onOpenMergeQueue: () => void }) {
  const titleId = useId();
  const hot = column.id === "attention" && tasks.length > 0;
  return (
    <section className={`board-col board-col--${column.id}${hot ? " board-col--hot" : ""}`} aria-labelledby={titleId}>
      <h3 className="board-col__title">
        {column.status && <StatusIcon status={column.status} decorative />}
        <span id={titleId}>{column.title}</span>
        <span className="board-col__count">{tasks.length}</span>
      </h3>
      {tasks.length === 0 ? (
        <p className="board-col__empty">{column.empty}</p>
      ) : (
        <ul className="board-col__cards">
          {tasks.map((task) => (
            <li key={task.id}>
              <BoardCard task={task} {...actions} />
            </li>
          ))}
        </ul>
      )}
      {column.id === "ready" && tasks.length > 0 && (
        <button type="button" className="wb-btn board-col__foot" onClick={onOpenMergeQueue}>
          <MergeIcon /> Merge queue
        </button>
      )}
    </section>
  );
}

function BoardCard({ task, onSelect, onOpenDiff, onContinue, queueRunning }: CardActions & { task: TaskDto }) {
  const [handingOff, setHandingOff] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);
  const id = useId();

  const status = taskStatus(task);
  const merged = Boolean(task.merged_into);
  const running = task.state === "running";
  const files = task.changed_files_total ?? task.changed_files.length;
  const uncommitted = task.uncommitted_count ?? 0;
  const conflicts = task.claim_conflicts?.length ?? 0;
  const overlaps = merged ? [] : (task.overlaps ?? []);
  const otherAgent: TaskAgent = task.agent === "codex" ? "claude_code" : "codex";
  const canHandOff = !running && !merged && USAGE_LIMIT.test(task.attention ?? "");

  // What needs you, what it is doing now, or what is left to review. Ready work
  // needs no line: its commits are in the meta.
  const why = merged
    ? null
    : (task.attention ??
      (conflicts > 0
        ? "Changed files another agent owns"
        : task.state === "failed"
          ? stateLabel(task)
          : running
            ? task.last_activity
            : readyToMerge(task)
              ? null
              : uncommitted > 0
                ? `${uncommitted} uncommitted ${uncommitted === 1 ? "change" : "changes"}`
                : task.last_activity));
  const meta = [
    AGENT_LABELS[task.agent],
    merged ? `Into ${task.merged_into}` : null,
    !merged && !running && task.commits_ahead > 0
      ? `${task.commits_ahead} ${task.commits_ahead === 1 ? "commit" : "commits"}`
      : null,
    files > 0 ? `${files} ${files === 1 ? "file" : "files"}` : null,
    formatRelative(task.created_at_ms),
  ].filter((part): part is string => part !== null);
  const owns = running ? task.claims?.[0] : undefined;

  async function handOff() {
    setHandingOff(true);
    setError(null);
    try {
      // The same handoff the orchestrator makes on its own; the new agent starts on its default model.
      await onContinue({ id: task.id, prompt: usageLimitHandoffPrompt(task.title), agent: otherAgent, model: "" });
    } catch (e) {
      if (mountedRef.current) setError(errorText(e));
    } finally {
      if (mountedRef.current) setHandingOff(false);
    }
  }

  const described = [why ? `${id}-why` : null, `${id}-meta`].filter(Boolean).join(" ");
  return (
    <div className={`board-card board-card--${status}`}>
      <button
        type="button"
        className="board-card__main"
        aria-label={task.title}
        aria-describedby={described}
        title={formatTime(task.created_at_ms)}
        onClick={() => onSelect(task.id)}
      >
        <span className="board-card__title">{task.title}</span>
        {why && (
          <span className={`board-card__why${running ? " activity__shimmer" : ""}`} id={`${id}-why`}>
            {why}
          </span>
        )}
        {overlaps.length > 0 && (
          <span className="board-card__warn">
            <WarningIcon />
            Overlaps {overlaps.map((o) => o.title).join(", ")}
          </span>
        )}
        <span className="board-card__meta" id={`${id}-meta`}>
          {meta.map((part) => (
            <span key={part}>{part}</span>
          ))}
          {owns && (
            <span>
              owns <span className="mono">{owns}</span>
            </span>
          )}
        </span>
      </button>
      {(canHandOff || conflicts > 0) && (
        <div className="board-card__actions">
          {canHandOff && (
            <button
              type="button"
              className="wb-btn wb-btn--primary"
              disabled={handingOff || queueRunning(task.repo_path)}
              onClick={() => void handOff()}
            >
              {handingOff ? "Handing off…" : `Hand off to ${AGENT_SHORT[otherAgent]}`}
            </button>
          )}
          {conflicts > 0 && (
            <button type="button" className="wb-btn" onClick={() => onOpenDiff(task.id)}>
              Review changes
            </button>
          )}
        </div>
      )}
      {error && (
        <p className="wb-note wb-note--error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/** Board / List: how the Ember overview lays out agents. */
export function ViewSwitch({ view, onChange }: { view: HomeView; onChange: (view: HomeView) => void }) {
  return (
    <div className="view-switch" role="group" aria-label="Overview layout">
      <button
        type="button"
        className="view-switch__btn"
        aria-pressed={view === "board"}
        onClick={() => onChange("board")}
      >
        Board
      </button>
      <button type="button" className="view-switch__btn" aria-pressed={view === "list"} onClick={() => onChange("list")}>
        List
      </button>
    </div>
  );
}
