import { memo, useEffect, useRef, useState, type FormEvent } from "react";

import { formatRelative, formatTime } from "../../format";
import {
  createTask,
  type CreateTaskRequest,
  type TaskAgent,
  type TaskDto,
  type TaskPermission,
} from "../../ipc";
import { CommitIcon, DiffIcon, MergeIcon, OverlapIcon, RevealIcon, StopIcon, TrashIcon, WarningIcon } from "./icons";
import { SidebarHeader } from "./Sidebar";
import { AGENT_LABELS, baseName, errorText, parseClaims, stateLabel } from "./state";

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
  onCreated: (task: TaskDto) => void;
  onOpenFolder: () => void;
  /** Changes whenever something asks to focus the new-agent form. */
  focusToken: number;
  /** Show tasks from every repository, not only the open workspace. */
  allRepos: boolean;
  onAllReposChange: (all: boolean) => void;
  /** Tasks hidden by the repository scope, for the toggle's label. */
  hiddenCount: number;
};

/** The Agents sidebar view: launch an agent in the workspace and watch every task. */
export default function AgentsPanel(props: Props) {
  const { tasks, listError, loadWarning, selectedTaskId, allRepos, hiddenCount } = props;
  return (
    <section className="wb-view wb-view--agents" aria-label="Agents">
      <SidebarHeader title="Agents">
        <button
          type="button"
          className={`scope-toggle${allRepos ? " scope-toggle--on" : ""}`}
          aria-pressed={allRepos}
          title={
            allRepos
              ? "Showing tasks from every repository"
              : "Showing only tasks in the open repository"
          }
          onClick={() => props.onAllReposChange(!allRepos)}
        >
          All repositories
          {!allRepos && hiddenCount > 0 && <span className="scope-toggle__count">{hiddenCount}</span>}
        </button>
      </SidebarHeader>
      <div className="wb-view__body">
        {loadWarning && (
          <p className="wb-note wb-note--error" role="alert">
            {loadWarning}
          </p>
        )}
        <NewAgentForm
          workspace={props.workspace}
          onCreated={props.onCreated}
          onOpenFolder={props.onOpenFolder}
          focusToken={props.focusToken}
        />
        <div className="agents-pane">
          <h3 className="wb-subhead">Tasks</h3>
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
            <ul className="agents" aria-label="Tasks">
              {tasks.map((task) => (
                <TaskRow
                  key={task.id}
                  task={task}
                  selected={task.id === selectedTaskId}
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

function NewAgentForm({
  workspace,
  onCreated,
  onOpenFolder,
  focusToken,
}: {
  workspace: string | null;
  onCreated: (task: TaskDto) => void;
  onOpenFolder: () => void;
  focusToken: number;
}) {
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [agent, setAgent] = useState<TaskAgent>("claude_code");
  const [permission, setPermission] = useState<TaskPermission>("edits");
  const [claimsText, setClaimsText] = useState("");
  const [autoHandoff, setAutoHandoff] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (focusToken > 0) titleRef.current?.focus();
  }, [focusToken]);

  const claims = parseClaims(claimsText);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!workspace) return;
    const request: CreateTaskRequest = {
      repo_path: workspace,
      title: title.trim(),
      prompt: prompt.trim(),
      agent,
      permission,
      auto_handoff: autoHandoff,
    };
    if (claims.length > 0) request.claims = claims;
    if (!request.title || !request.prompt) {
      setError("Title and prompt are required.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const task = await createTask(request);
      setTitle("");
      setPrompt("");
      setClaimsText("");
      onCreated(task);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  const disabled = !workspace;
  return (
    <form className="new-agent" aria-label="New agent" onSubmit={(e) => void submit(e)}>
      <h3 className="wb-subhead">New agent</h3>
      {disabled && (
        <p className="wb-note">
          Open a folder to launch agents in it.{" "}
          <button type="button" className="wb-link" onClick={onOpenFolder}>
            Open Folder…
          </button>
        </p>
      )}
      <fieldset disabled={disabled} className="new-agent__fields">
        <div className="new-agent__row">
          <label className="wb-field new-agent__title">
            <span>Title</span>
            <input ref={titleRef} type="text" value={title} onChange={(e) => setTitle(e.target.value)} />
          </label>
          <label className="wb-field">
            <span>Agent</span>
            <select value={agent} onChange={(e) => setAgent(e.target.value as TaskAgent)}>
              <option value="claude_code">{AGENT_LABELS.claude_code}</option>
              <option value="codex">{AGENT_LABELS.codex}</option>
            </select>
          </label>
        </div>
        <label className="wb-field">
          <span>Prompt</span>
          <textarea
            rows={4}
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                e.currentTarget.form?.requestSubmit();
              }
            }}
          />
        </label>
        <div className="new-agent__permission">
          <div className="segmented" role="radiogroup" aria-label="Permission">
            {(Object.keys(PERMISSION_LABELS) as TaskPermission[]).map((value) => (
              <button
                key={value}
                type="button"
                role="radio"
                aria-checked={permission === value}
                className={`segmented__option${permission === value ? " segmented__option--on" : ""}`}
                title={PERMISSION_HINTS[value]}
                onClick={() => setPermission(value)}
              >
                {PERMISSION_LABELS[value]}
              </button>
            ))}
          </div>
          <span className="new-agent__hint">{PERMISSION_HINTS[permission]}</span>
        </div>
        <label className="wb-field">
          <span>Owns (files or folders)</span>
          <textarea
            rows={2}
            className="new-agent__claims"
            placeholder="e.g. calc.py, greet.py"
            value={claimsText}
            onChange={(e) => setClaimsText(e.target.value)}
          />
        </label>
        {claims.length > 0 && (
          <ul className="claim-chips" aria-label="Owned paths">
            {claims.map((claim) => (
              <li key={claim} className="claim-chip mono">
                {claim}
              </li>
            ))}
          </ul>
        )}
        <p className="new-agent__hint">
          Other agents are told not to touch these. A commit that changes another agent&rsquo;s files needs
          confirmation. End a folder with <span className="mono">/</span>; separate with commas or new lines.
        </p>
        <label className="new-agent__check">
          <input type="checkbox" checked={autoHandoff} onChange={(e) => setAutoHandoff(e.target.checked)} />
          <span>Auto-handoff on usage limit</span>
        </label>
        {error && (
          <p className="wb-note wb-note--error" role="alert">
            {error}
          </p>
        )}
        <div className="new-agent__actions">
          {workspace && (
            <span className="new-agent__repo" title={workspace}>
              in {baseName(workspace)}
            </span>
          )}
          <button type="submit" className="wb-btn wb-btn--primary" disabled={busy} title="Launch (⌘Enter)">
            {busy ? "Launching…" : "Launch"}
          </button>
        </div>
      </fieldset>
    </form>
  );
}

const PERMISSION_LABELS: Record<TaskPermission, string> = { edits: "Edits", auto: "Auto" };
const PERMISSION_HINTS: Record<TaskPermission, string> = {
  edits: "Claude may edit files but not run shell commands.",
  auto: "Claude's safety classifier approves routine actions like running tests.",
};

type RowProps = TaskActions & { task: TaskDto; selected: boolean };

const TaskRow = memo(function TaskRow({
  task,
  selected,
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

  return (
    <li
      className={`agent-row${selected ? " agent-row--selected" : ""}${task.state === "running" ? " agent-row--running" : ""}`}
      aria-label={task.title}
    >
      <button
        type="button"
        className="agent-row__main"
        aria-current={selected || undefined}
        onClick={() => onSelect(task.id)}
      >
        <span className="agent-row__head">
          <AgentChip task={task} />
          <span className="agent-row__title">{task.title}</span>
          {task.attention && (
            <span className="row-flag row-flag--attention" title={task.attention} aria-label="Needs attention">
              <WarningIcon />
            </span>
          )}
          {uncommitted > 0 && !task.merged_into && (
            <span
              className="row-flag row-flag--uncommitted"
              title={`${uncommitted} uncommitted ${uncommitted === 1 ? "change" : "changes"}`}
              aria-label={`${uncommitted} uncommitted`}
            >
              <CommitIcon />
              {uncommitted}
            </span>
          )}
          {overlapCount > 0 && (
            <span
              className="row-flag row-flag--overlap"
              title={`Touches the same files as ${task.overlaps!.map((o) => o.title).join(", ")}`}
              aria-label={`Overlaps with ${overlapCount} ${overlapCount === 1 ? "task" : "tasks"}`}
            >
              <OverlapIcon />
              {overlapCount}
            </span>
          )}
          {task.auto_handoff === false && !task.merged_into && (
            <span
              className="row-flag row-flag--manual"
              title="Auto-handoff is off: this task waits for you when the agent hits a usage limit."
            >
              manual handoff
            </span>
          )}
          {task.merged_into && (
            <span
              className="row-flag row-flag--merged"
              title={`Merged into ${task.merged_into}`}
              aria-label={`Merged into ${task.merged_into}`}
            >
              <MergeIcon />
            </span>
          )}
          <time className="agent-row__time" title={formatTime(task.created_at_ms)}>
            {formatRelative(task.created_at_ms)}
          </time>
        </span>
        <span className="agent-row__meta">
          <StateBadge task={task} />
          <span>{AGENT_LABELS[task.agent] ?? task.agent}</span>
          <span className="mono agent-row__branch" title={task.worktree_path}>
            {task.branch}
          </span>
        </span>
        <span className="agent-row__meta">
          <span>
            {task.commits_ahead} {task.commits_ahead === 1 ? "commit" : "commits"} ahead
          </span>
          <span>
            {fileCount} {fileCount === 1 ? "file" : "files"} changed
          </span>
        </span>
        {task.last_activity && (
          <span className="mono agent-row__activity" title={task.last_activity}>
            {task.last_activity}
          </span>
        )}
      </button>
      {error && (
        <p className="wb-note wb-note--error" role="alert">
          {error}
        </p>
      )}
      {confirming ? (
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
      ) : (
        <div className="agent-row__actions">
          {task.state === "running" && (
            <button
              type="button"
              className="wb-icon-btn"
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
            aria-label="Open diff"
            title="Open diff"
            onClick={() => onOpenDiff(task.id)}
          >
            <DiffIcon />
          </button>
          <button
            type="button"
            className="wb-icon-btn"
            aria-label="Reveal in Finder"
            title="Reveal in Finder"
            onClick={() => void act(onReveal)}
          >
            <RevealIcon />
          </button>
          <button
            type="button"
            className="wb-icon-btn wb-icon-btn--danger"
            aria-label="Discard"
            title="Discard"
            disabled={busy}
            onClick={() => setConfirming(true)}
          >
            <TrashIcon />
          </button>
        </div>
      )}
    </li>
  );
});

/** A per-agent monogram chip (decorative; the agent name is shown as text nearby). */
export function AgentChip({ task, large = false }: { task: TaskDto; large?: boolean }) {
  const running = task.state === "running";
  return (
    <span
      className={`agent-chip agent-chip--${task.agent}${large ? " agent-chip--lg" : ""}${running ? " agent-chip--running" : ""}`}
      aria-hidden="true"
    >
      {task.agent === "codex" ? "X" : "C"}
    </span>
  );
}

export function StateBadge({ task }: { task: TaskDto }) {
  return (
    <span className={`state-badge state-badge--${task.state}`}>
      <span className="state-badge__dot" aria-hidden="true" />
      {stateLabel(task)}
    </span>
  );
}
