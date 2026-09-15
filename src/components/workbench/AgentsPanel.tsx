import { memo, useEffect, useRef, useState, type FormEvent } from "react";

import { formatRelative, formatTime } from "../../format";
import {
  createTask,
  type CreateTaskRequest,
  type TaskAgent,
  type TaskDto,
  type TaskEffort,
  type TaskPermission,
} from "../../ipc";
import AgentMenu from "./AgentMenu";
import { ArrowUpIcon, DiffIcon, RevealIcon, StopIcon, TrashIcon, WarningIcon } from "./icons";
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
  const [model, setModel] = useState<string | null>(null);
  const [effort, setEffort] = useState<TaskEffort | null>(null);
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
    if (model) request.model = model;
    if (effort) request.effort = effort;
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
      {disabled && (
        <p className="wb-note">
          Open a folder to launch agents in it.{" "}
          <button type="button" className="wb-link" onClick={onOpenFolder}>
            Open Folder…
          </button>
        </p>
      )}
      <fieldset disabled={disabled} className="new-agent__fields">
        <div className="composer__box">
          <input
            ref={titleRef}
            className="new-agent__name"
            type="text"
            aria-label="Title"
            placeholder="Title"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
          />
          <textarea
            className="composer__input"
            aria-label="Prompt"
            rows={3}
            placeholder="What should the agent do?"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                e.currentTarget.form?.requestSubmit();
              }
            }}
          />
          <details className="new-agent__more">
            <summary>Owns paths</summary>
            <textarea
              rows={2}
              className="new-agent__claims"
              aria-label="Owns (files or folders)"
              placeholder="e.g. calc.py, greet.py"
              value={claimsText}
              onChange={(e) => setClaimsText(e.target.value)}
            />
            <p className="new-agent__hint">
              Other agents are told not to touch these. A commit that changes another agent&rsquo;s files needs
              confirmation. End a folder with <span className="mono">/</span>; separate with commas or new lines.
            </p>
          </details>
          {claims.length > 0 && (
            <ul className="claim-chips" aria-label="Owned paths">
              {claims.map((claim) => (
                <li key={claim} className="claim-chip mono">
                  {claim}
                </li>
              ))}
            </ul>
          )}
          {error && (
            <p className="wb-note wb-note--error" role="alert">
              {error}
            </p>
          )}
          <div className="composer__bar">
            <AgentMenu
              label="Agent"
              agent={agent}
              onAgentChange={setAgent}
              permission={permission}
              onPermissionChange={setPermission}
              autoHandoff={autoHandoff}
              onAutoHandoffChange={setAutoHandoff}
              model={model}
              onModelChange={setModel}
              effort={effort}
              onEffortChange={setEffort}
              disabled={disabled}
            />
            {workspace && (
              <span className="new-agent__repo" title={workspace}>
                {baseName(workspace)}
              </span>
            )}
            <button
              type="submit"
              className="composer__send"
              disabled={busy}
              title="Launch (⌘Enter)"
              aria-label={busy ? "Launching…" : "Launch"}
            >
              <ArrowUpIcon />
            </button>
          </div>
        </div>
      </fieldset>
    </form>
  );
}

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
  const hint = [
    task.branch,
    `${task.commits_ahead} ${task.commits_ahead === 1 ? "commit" : "commits"} ahead · ${fileCount} ${fileCount === 1 ? "file" : "files"} changed`,
    uncommitted > 0 && !task.merged_into
      ? `${uncommitted} uncommitted ${uncommitted === 1 ? "change" : "changes"}`
      : null,
    overlapCount > 0
      ? `Overlaps ${overlapCount} ${overlapCount === 1 ? "task" : "tasks"}: ${task.overlaps!.map((o) => o.title).join(", ")}`
      : null,
    task.merged_into ? `Merged into ${task.merged_into}` : null,
    task.auto_handoff === false && !task.merged_into ? "manual handoff" : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <li
      className={`agent-row${selected ? " agent-row--selected" : ""}`}
      aria-label={task.title}
    >
      <button
        type="button"
        className="agent-row__main"
        title={hint}
        aria-label={task.title}
        aria-current={selected || undefined}
        onClick={() => onSelect(task.id)}
      >
        <span className="agent-row__head">
          <span className="agent-row__title">{task.title}</span>
        </span>
        <span className="agent-row__meta">
          {task.merged_into ? (
            <span className="state-badge state-badge--finished">
              <span className="state-badge__dot" aria-hidden="true" />
              Merged
            </span>
          ) : (
            <StateBadge task={task} compact />
          )}
          <span>{AGENT_LABELS[task.agent] ?? task.agent}</span>
        </span>
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
        <div className="agent-row__end">
          {task.attention && (
            <span className="row-flag row-flag--attention" title={task.attention} aria-label="Needs attention">
              <WarningIcon />
            </span>
          )}
          <time className="agent-row__time" title={formatTime(task.created_at_ms)}>
            {formatRelative(task.created_at_ms)}
          </time>
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
        </div>
      )}
    </li>
  );
});

/** A small status mark: a dot plus the state word, used on rows and the agent header. */
export function StateBadge({ task, compact }: { task: TaskDto; compact?: boolean }) {
  return (
    <span className={`state-badge state-badge--${task.state}`}>
      <span className="state-badge__dot" aria-hidden="true" />
      {compact && task.state === "failed" ? "Failed" : stateLabel(task)}
    </span>
  );
}
