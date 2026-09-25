import { useEffect, useState, type FormEvent } from "react";

import type { ContinueTaskRequest, MergeResultDto, TaskAgent, TaskDto, TaskEffort } from "../../ipc";
import ActivityList from "./ActivityList";
import AgentMenu from "./AgentMenu";
import { StateBadge } from "./badges";
import {
  CommitIcon,
  DiffIcon,
  MergeIcon,
  OverlapIcon,
  RevealIcon,
  StopIcon,
  TrashIcon,
  WarningIcon,
  ArrowUpIcon,
} from "./icons";
import { errorText, isOwnershipError, shortcut } from "./state";
import { useActivity } from "./useTasks";

type Props = {
  /** Only the visible tab polls for activity. */
  active?: boolean;
  taskId: string;
  task: TaskDto | undefined;
  onStop: (id: string) => Promise<void>;
  onDiscard: (id: string) => Promise<void>;
  onReveal: (id: string) => Promise<void>;
  onOpenDiff: (id: string) => void;
  onContinue: (request: ContinueTaskRequest) => Promise<void>;
  onCommit: (id: string, message: string, force?: boolean) => Promise<void>;
  onMerge: (id: string) => Promise<MergeResultDto>;
  /** Open or focus another task's agent tab. */
  onSelectTask: (id: string) => void;
  /** A merge queue is running in this task's repository. */
  mergeQueueRunning?: boolean;
};

/** Short agent names for handoff labels. */
const AGENT_SHORT: Record<TaskAgent, string> = { claude_code: "Claude", codex: "Codex" };

type MergeOutcome = { kind: "result"; result: MergeResultDto } | { kind: "error"; message: string };

/** Editor tab for one agent: status, git actions, activity timeline, and a composer to continue. */
export default function AgentView({
  active = true,
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
  mergeQueueRunning = false,
}: Props) {
  const activity = useActivity(task && active ? taskId : null, task?.state === "running");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [commitMessage, setCommitMessage] = useState("");
  const [committing, setCommitting] = useState(false);
  const [commitError, setCommitError] = useState<string | null>(null);
  /** Set when the backend refused a commit because another task owns the files. */
  const [commitBlocked, setCommitBlocked] = useState(false);
  const [confirmingForce, setConfirmingForce] = useState(false);

  const [confirmingMerge, setConfirmingMerge] = useState(false);
  const [merging, setMerging] = useState(false);
  const [mergeOutcome, setMergeOutcome] = useState<MergeOutcome | null>(null);

  const currentAgent = task?.agent;
  const [prompt, setPrompt] = useState("");
  const [nextAgent, setNextAgent] = useState<TaskAgent>(currentAgent ?? "claude_code");
  const [nextModel, setNextModel] = useState<string | null>(task?.model ?? null);
  const [nextEffort, setNextEffort] = useState<TaskEffort | null>(task?.effort ?? null);
  const [continuing, setContinuing] = useState(false);
  const [continueError, setContinueError] = useState<string | null>(null);

  // The selector defaults to whichever agent owns the task now (a handoff changes it).
  useEffect(() => {
    if (currentAgent) setNextAgent(currentAgent);
  }, [currentAgent]);
  useEffect(() => {
    setNextModel(task?.model ?? null);
    setNextEffort(task?.effort ?? null);
  }, [task?.model, task?.effort]);

  if (!task) {
    return <p className="wb-note wb-view-pad">This agent no longer exists.</p>;
  }

  const running = task.state === "running";
  const merged = Boolean(task.merged_into);
  const uncommitted = task.uncommitted_count ?? 0;
  const overlaps = task.overlaps ?? [];
  const claimConflicts = task.claim_conflicts ?? [];
  const claims = task.claims ?? [];
  const canCommit = !running && !merged && uncommitted > 0;
  const mergedNow = mergeOutcome?.kind === "result" && mergeOutcome.result.merged;
  const canMerge = !running && !merged && !mergedNow && uncommitted === 0 && task.commits_ahead > 0;
  const canContinue = !running && !merged;
  // The queue owns the repository's merge slot and may be updating this branch.
  const queueLocked = mergeQueueRunning && !running && !merged;
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

  async function commit(force = false) {
    if (queueLocked) return;
    setCommitting(true);
    setCommitError(null);
    try {
      await onCommit(taskId, commitMessage.trim(), force || undefined);
      setCommitMessage("");
      setCommitBlocked(false);
      setConfirmingForce(false);
    } catch (e) {
      const message = errorText(e);
      setCommitError(message);
      setCommitBlocked(isOwnershipError(message));
      setConfirmingForce(false);
    } finally {
      setCommitting(false);
    }
  }

  async function merge() {
    if (queueLocked) return;
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
    if (queueLocked) return;
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
      request.model = nextModel ?? "";
      if (nextEffort) request.effort = nextEffort;
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
          <h2 className="agent-view__title">{task.title}</h2>
          <StateBadge task={task} />
          {task.runs !== undefined && task.runs > 0 && <span className="agent-view__runs">Run {task.runs}</span>}
          {task.auto_handoff === false && (
            <span className="agent-view__runs" title="Auto-handoff is off: a usage limit waits for you.">
              manual handoff
            </span>
          )}
          <span className="agent-view__spacer" />
          {confirming ? (
            <>
              <span className="agent-view__confirm">Delete this worktree and branch?</span>
              <button
                type="button"
                className="wb-btn wb-btn--danger wb-btn--small"
                disabled={busy}
                onClick={() => void act(onDiscard)}
              >
                Confirm discard
              </button>
              <button type="button" className="wb-btn wb-btn--small" onClick={() => setConfirming(false)}>
                Cancel
              </button>
            </>
          ) : (
            <>
              {running && (
                <button
                  type="button"
                  className="wb-btn wb-btn--small"
                  disabled={busy}
                  onClick={() => void act(onStop)}
                >
                  <StopIcon /> Stop
                </button>
              )}
              <button type="button" className="wb-btn wb-btn--small" onClick={() => onOpenDiff(taskId)}>
                <DiffIcon /> Diff
              </button>
              <button type="button" className="wb-btn wb-btn--small" onClick={() => setConfirming(true)}>
                <TrashIcon /> Discard
              </button>
            </>
          )}
        </div>
        <div className="agent-view__meta">
          {claims.length > 0 && (
            <ul className="claim-chips" aria-label="Owned paths">
              {claims.map((claim) => (
                <li key={claim} className="claim-chip mono" title="Other agents are told not to touch this">
                  {claim}
                </li>
              ))}
            </ul>
          )}
          <span className="mono">{task.branch}</span>
          <span className="mono agent-view__worktree" title={task.worktree_path}>
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
        </div>
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

        {claimConflicts.length > 0 && (
          <div className="agent-callout agent-callout--claim" role="alert" aria-label="Claim conflicts">
            <WarningIcon />
            <div className="agent-callout__body">
              <span>
                This task changed files another agent owns. Committing them needs confirmation.
              </span>
              <ul className="overlap-list">
                {claimConflicts.map((conflict) => (
                  <li key={conflict.task_id}>
                    <span>
                      Changed files owned by{" "}
                      <button type="button" className="wb-link" onClick={() => onSelectTask(conflict.task_id)}>
                        {conflict.title}
                      </button>
                      :
                    </span>
                    <span className="overlap-list__files mono">{conflict.files.join(", ")}</span>
                  </li>
                ))}
              </ul>
            </div>
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

        {queueLocked && (
          <p className="wb-note queue-lock" role="status">
            Commit, merge, and continue are paused while this repository&rsquo;s merge queue runs.
          </p>
        )}

        {(canCommit || canMerge || mergeOutcome) && (
          <div className="git-actions">
            {canCommit && (
              <form
                className="git-actions__row"
                aria-label="Commit changes"
                onSubmit={(e) => {
                  e.preventDefault();
                  void commit();
                }}
              >
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
                <button type="submit" className="wb-btn" disabled={committing || queueLocked}>
                  {committing ? "Committing…" : "Commit"}
                </button>
              </form>
            )}
            {commitError && (
              <p className="wb-note wb-note--error" role="alert">
                {commitError}
              </p>
            )}
            {commitBlocked &&
              (confirmingForce ? (
                <div className="git-actions__row">
                  <span className="git-actions__label">
                    Commit these files even though another agent owns them?
                  </span>
                  <button
                    type="button"
                    className="wb-btn wb-btn--danger"
                    disabled={committing || queueLocked}
                    onClick={() => void commit(true)}
                  >
                    {committing ? "Committing…" : "Confirm commit"}
                  </button>
                  <button
                    type="button"
                    className="wb-btn"
                    disabled={committing}
                    onClick={() => setConfirmingForce(false)}
                  >
                    Cancel
                  </button>
                </div>
              ) : (
                <div className="git-actions__row">
                  <button
                    type="button"
                    className="wb-btn"
                    disabled={queueLocked}
                    onClick={() => setConfirmingForce(true)}
                  >
                    Commit anyway
                  </button>
                </div>
              ))}
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
                      disabled={merging || queueLocked}
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
                      disabled={queueLocked}
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
          <div className="composer__box">
            <textarea
              className="composer__input"
              aria-label="Follow-up prompt"
              rows={2}
              placeholder={handoff ? `Brief ${AGENT_SHORT[nextAgent]} on what to do next…` : "Tell the agent what to do next…"}
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
              <AgentMenu
                label="Next agent"
                agent={nextAgent}
                onAgentChange={setNextAgent}
                model={nextModel}
                onModelChange={setNextModel}
                effort={nextEffort}
                onEffortChange={setNextEffort}
                disabled={continuing}
                placement="up"
              />
              {(queueLocked || handoff) && (
                <span className="composer__hint">
                  {queueLocked
                    ? "Paused while the merge queue runs"
                    : "Lore passes a context brief to the new agent."}
                </span>
              )}
              <button
                type="submit"
                className="composer__send"
                disabled={continuing || queueLocked}
                title={`Send (${shortcut("Enter")})`}
                aria-label={continuing ? "Sending…" : handoff ? `Hand off to ${AGENT_SHORT[nextAgent]}` : "Continue"}
              >
                <ArrowUpIcon />
              </button>
            </div>
          </div>
        </form>
      )}
    </div>
  );
}
