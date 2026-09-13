import { useEffect, useState, type FormEvent } from "react";

import type { ContinueTaskRequest, MergeResultDto, TaskAgent, TaskDto } from "../../ipc";
import ActivityList from "./ActivityList";
import { AgentChip, StateBadge } from "./AgentsPanel";
import {
  CommitIcon,
  DiffIcon,
  MergeIcon,
  OverlapIcon,
  RevealIcon,
  StopIcon,
  TrashIcon,
  WarningIcon,
} from "./icons";
import { AGENT_LABELS, errorText } from "./state";
import { useActivity } from "./useTasks";

type Props = {
  taskId: string;
  task: TaskDto | undefined;
  onStop: (id: string) => Promise<void>;
  onDiscard: (id: string) => Promise<void>;
  onReveal: (id: string) => Promise<void>;
  onOpenDiff: (id: string) => void;
  onContinue: (request: ContinueTaskRequest) => Promise<void>;
  onCommit: (id: string, message: string) => Promise<void>;
  onMerge: (id: string) => Promise<MergeResultDto>;
  /** Open or focus another task's agent tab. */
  onSelectTask: (id: string) => void;
};

/** Short agent names for handoff labels. */
const AGENT_SHORT: Record<TaskAgent, string> = { claude_code: "Claude", codex: "Codex" };

type MergeOutcome = { kind: "result"; result: MergeResultDto } | { kind: "error"; message: string };

/** Editor tab for one agent: status, git actions, activity timeline, and a composer to continue. */
export default function AgentView({
  taskId,
  task,
  onStop,
  onDiscard,
  onReveal,
  onOpenDiff,
  onContinue,
  onCommit,
  onMerge,
  onSelectTask,
}: Props) {
  const activity = useActivity(task ? taskId : null, task?.state === "running");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [commitMessage, setCommitMessage] = useState("");
  const [committing, setCommitting] = useState(false);
  const [commitError, setCommitError] = useState<string | null>(null);

  const [confirmingMerge, setConfirmingMerge] = useState(false);
  const [merging, setMerging] = useState(false);
  const [mergeOutcome, setMergeOutcome] = useState<MergeOutcome | null>(null);

  const currentAgent = task?.agent;
  const [prompt, setPrompt] = useState("");
  const [nextAgent, setNextAgent] = useState<TaskAgent>(currentAgent ?? "claude_code");
  const [continuing, setContinuing] = useState(false);
  const [continueError, setContinueError] = useState<string | null>(null);

  // The selector defaults to whichever agent owns the task now (a handoff changes it).
  useEffect(() => {
    if (currentAgent) setNextAgent(currentAgent);
  }, [currentAgent]);

  if (!task) {
    return <p className="wb-note wb-view-pad">This agent no longer exists.</p>;
  }

  const running = task.state === "running";
  const merged = Boolean(task.merged_into);
  const uncommitted = task.uncommitted_count ?? 0;
  const overlaps = task.overlaps ?? [];
  const canCommit = !running && !merged && uncommitted > 0;
  const mergedNow = mergeOutcome?.kind === "result" && mergeOutcome.result.merged;
  const canMerge = !running && !merged && !mergedNow && uncommitted === 0 && task.commits_ahead > 0;
  const canContinue = !running && !merged;
  const handoff = nextAgent !== task.agent;

  async function act(action: (id: string) => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await action(taskId);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  async function commit(event: FormEvent) {
    event.preventDefault();
    setCommitting(true);
    setCommitError(null);
    try {
      await onCommit(taskId, commitMessage.trim());
      setCommitMessage("");
    } catch (e) {
      setCommitError(errorText(e));
    } finally {
      setCommitting(false);
    }
  }

  async function merge() {
    setMerging(true);
    setMergeOutcome(null);
    try {
      const result = await onMerge(taskId);
      setMergeOutcome({ kind: "result", result });
    } catch (e) {
      setMergeOutcome({ kind: "error", message: errorText(e) });
    } finally {
      setMerging(false);
      setConfirmingMerge(false);
    }
  }

  async function submitContinue(event: FormEvent) {
    event.preventDefault();
    const text = prompt.trim();
    if (!text) {
      setContinueError("Describe what the agent should do next.");
      return;
    }
    setContinuing(true);
    setContinueError(null);
    try {
      const request: ContinueTaskRequest = { id: taskId, prompt: text };
      if (handoff) request.agent = nextAgent;
      await onContinue(request);
      setPrompt("");
    } catch (e) {
      setContinueError(errorText(e));
    } finally {
      setContinuing(false);
    }
  }

  return (
    <div className="agent-view">
      <header className="agent-view__header">
        <div className="agent-view__titleline">
          <AgentChip task={task} large />
          <h2 className="agent-view__title">{task.title}</h2>
          <StateBadge task={task} />
          {task.merged_into && (
            <span className="merged-badge">
              <MergeIcon /> Merged into {task.merged_into}
            </span>
          )}
          <span className="agent-view__agent">{AGENT_LABELS[task.agent] ?? task.agent}</span>
          {task.runs !== undefined && task.runs > 0 && <span className="agent-view__runs">Run {task.runs}</span>}
          <span className="agent-view__spacer" />
          {confirming ? (
            <>
              <span className="agent-view__confirm">Delete this worktree and branch?</span>
              <button
                type="button"
                className="wb-btn wb-btn--danger"
                disabled={busy}
                onClick={() => void act(onDiscard)}
              >
                Confirm discard
              </button>
              <button type="button" className="wb-btn" onClick={() => setConfirming(false)}>
                Cancel
              </button>
            </>
          ) : (
            <>
              {running && (
                <button type="button" className="wb-btn" disabled={busy} onClick={() => void act(onStop)}>
                  <StopIcon /> Stop
                </button>
              )}
              <button type="button" className="wb-btn" onClick={() => onOpenDiff(taskId)}>
                <DiffIcon /> Diff
              </button>
              <button type="button" className="wb-btn" onClick={() => setConfirming(true)}>
                <TrashIcon /> Discard
              </button>
            </>
          )}
        </div>
        <dl className="agent-view__meta">
          <div>
            <dt>Branch</dt>
            <dd className="mono">{task.branch}</dd>
          </div>
          <div>
            <dt>Commits ahead</dt>
            <dd>{task.commits_ahead}</dd>
          </div>
          <div>
            <dt>Changed files</dt>
            <dd>{task.changed_files.length}</dd>
          </div>
          {!merged && (
            <div>
              <dt>Uncommitted</dt>
              <dd>{uncommitted}</dd>
            </div>
          )}
          <div className="agent-view__worktree">
            <dt>Worktree</dt>
            <dd>
              <span className="mono" title={task.worktree_path}>
                {task.worktree_path}
              </span>
              <button
                type="button"
                className="wb-icon-btn"
                aria-label="Reveal in Finder"
                title="Reveal in Finder"
                onClick={() => void act(onReveal)}
              >
                <RevealIcon />
              </button>
            </dd>
          </div>
        </dl>
        {error && (
          <p className="wb-note wb-note--error" role="alert">
            {error}
          </p>
        )}

        {task.attention && (
          <div className="agent-callout agent-callout--attention" role="alert" aria-label="Needs attention">
            <WarningIcon />
            <span>{task.attention}</span>
          </div>
        )}

        {overlaps.length > 0 && (
          <div className="agent-callout agent-callout--overlap" role="region" aria-label="Overlapping tasks">
            <OverlapIcon />
            <div className="agent-callout__body">
              <span>
                {overlaps.length === 1 ? "Another unmerged task changes" : "Other unmerged tasks change"} the same
                files. Merging both may conflict.
              </span>
              <ul className="overlap-list">
                {overlaps.map((overlap) => (
                  <li key={overlap.task_id}>
                    <button type="button" className="wb-link" onClick={() => onSelectTask(overlap.task_id)}>
                      {overlap.title}
                    </button>
                    <span className="overlap-list__files mono">{overlap.files.join(", ")}</span>
                  </li>
                ))}
              </ul>
            </div>
          </div>
        )}

        {(canCommit || canMerge || mergeOutcome) && (
          <div className="git-actions">
            {canCommit && (
              <form className="git-actions__row" aria-label="Commit changes" onSubmit={(e) => void commit(e)}>
                <span className="git-actions__label">
                  <CommitIcon /> {uncommitted} uncommitted {uncommitted === 1 ? "change" : "changes"}
                </span>
                <input
                  type="text"
                  className="wb-select git-actions__input"
                  aria-label="Commit message"
                  placeholder={task.title}
                  value={commitMessage}
                  disabled={committing}
                  onChange={(e) => setCommitMessage(e.target.value)}
                />
                <button type="submit" className="wb-btn" disabled={committing}>
                  {committing ? "Committing…" : "Commit"}
                </button>
              </form>
            )}
            {commitError && (
              <p className="wb-note wb-note--error" role="alert">
                {commitError}
              </p>
            )}
            {canMerge && (
              <div className="git-actions__row">
                {confirmingMerge ? (
                  <>
                    <span className="git-actions__label">
                      Merge <span className="mono">{task.branch}</span> into{" "}
                      {task.repo_branch ? <span className="mono">{task.repo_branch}</span> : "your checked-out branch"}?
                    </span>
                    <button
                      type="button"
                      className="wb-btn wb-btn--primary"
                      disabled={merging}
                      onClick={() => void merge()}
                    >
                      {merging ? "Merging…" : "Confirm merge"}
                    </button>
                    <button
                      type="button"
                      className="wb-btn"
                      disabled={merging}
                      onClick={() => setConfirmingMerge(false)}
                    >
                      Cancel
                    </button>
                  </>
                ) : (
                  <>
                    <span className="git-actions__label">
                      <MergeIcon /> {task.commits_ahead} {task.commits_ahead === 1 ? "commit" : "commits"} ready
                    </span>
                    <button
                      type="button"
                      className="wb-btn"
                      onClick={() => {
                        setMergeOutcome(null);
                        setConfirmingMerge(true);
                      }}
                    >
                      {task.repo_branch ? `Merge into ${task.repo_branch}` : "Merge into checked-out branch"}
                    </button>
                  </>
                )}
              </div>
            )}
            {mergeOutcome?.kind === "error" && (
              <p className="wb-note wb-note--error" role="alert">
                {mergeOutcome.message}
              </p>
            )}
            {mergeOutcome?.kind === "result" && mergeOutcome.result.merged && (
              <p className="merge-note merge-note--ok" role="status">
                {mergeOutcome.result.message}
              </p>
            )}
            {mergeOutcome?.kind === "result" && !mergeOutcome.result.merged && (
              <div className="merge-note merge-note--conflict" role="alert">
                <p>
                  Merge into <span className="mono">{mergeOutcome.result.into_branch}</span> hit conflicts and was
                  aborted. Nothing was changed in your checkout.
                </p>
                {mergeOutcome.result.message && <p className="merge-note__detail">{mergeOutcome.result.message}</p>}
                <ul aria-label="Conflicting files">
                  {mergeOutcome.result.conflicts.map((file) => (
                    <li key={file} className="mono">
                      {file}
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        )}
      </header>
      <ActivityList items={activity.items} error={activity.error} label={`Activity of ${task.title}`} />
      {canContinue && (
        <form className="composer" aria-label="Continue task" onSubmit={(e) => void submitContinue(e)}>
          <textarea
            className="composer__input"
            aria-label="Follow-up prompt"
            rows={2}
            placeholder={handoff ? `Brief ${AGENT_SHORT[nextAgent]} on what to do next…` : "What should the agent do next?"}
            value={prompt}
            disabled={continuing}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                e.currentTarget.form?.requestSubmit();
              }
            }}
          />
          {continueError && (
            <p className="wb-note wb-note--error" role="alert">
              {continueError}
            </p>
          )}
          <div className="composer__bar">
            <select
              className="wb-select composer__agent"
              aria-label="Next agent"
              value={nextAgent}
              disabled={continuing}
              onChange={(e) => setNextAgent(e.target.value as TaskAgent)}
            >
              <option value="claude_code">{AGENT_LABELS.claude_code}</option>
              <option value="codex">{AGENT_LABELS.codex}</option>
            </select>
            <span className="composer__hint">
              {handoff ? "Lore passes a context brief to the new agent." : "⌘Enter to send"}
            </span>
            <button type="submit" className="wb-btn wb-btn--primary" disabled={continuing} title="⌘Enter">
              {continuing ? "Sending…" : handoff ? `Hand off to ${AGENT_SHORT[nextAgent]}` : "Continue"}
            </button>
          </div>
        </form>
      )}
    </div>
  );
}
