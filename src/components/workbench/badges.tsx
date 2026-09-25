import type { TaskAgent, TaskDto } from "../../ipc";
import { AGENT_LABELS, stateLabel } from "./state";

export type Status = "running" | "attention" | "failed" | "finished" | "stopped" | "merged";

/** One status per task, in the order the user cares about. */
export function taskStatus(task: TaskDto): Status {
  if (task.merged_into) return "merged";
  if (task.state === "running") return "running";
  if (task.attention || (task.claim_conflicts?.length ?? 0) > 0) return "attention";
  if (task.state === "failed") return "failed";
  if (task.state === "finished") return "finished";
  return "stopped";
}

const STATUS_WORDS: Record<Status, string> = {
  running: "Running",
  attention: "Needs you",
  failed: "Failed",
  finished: "Finished",
  stopped: "Stopped",
  merged: "Merged",
};

/**
 * A 14px status glyph in the Linear idiom: a spinning arc while running, a
 * check when finished, a cross when failed, a filled mark when the user is
 * needed, the merge glyph once merged, a pause for stopped work.
 */
export function StatusIcon({ status, decorative = false }: { status: Status; decorative?: boolean }) {
  const a11y = decorative ? { "aria-hidden": true } : { role: "img", "aria-label": STATUS_WORDS[status] };
  return (
    <span className={`status-icon status-icon--${status}`} {...a11y}>
      <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true" focusable="false">
        {status === "running" && (
          <>
            <circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" strokeOpacity="0.25" strokeWidth="1.5" />
            <path d="M7 1.5a5.5 5.5 0 0 1 5.5 5.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" className="status-icon__arc" />
          </>
        )}
        {status === "finished" && (
          <>
            <circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" strokeWidth="1.5" />
            <path d="M4.6 7.1l1.6 1.6 3.2-3.3" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
          </>
        )}
        {status === "failed" && (
          <>
            <circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" strokeWidth="1.5" />
            <path d="M5 5l4 4M9 5l-4 4" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </>
        )}
        {status === "attention" && (
          <>
            <circle cx="7" cy="7" r="6.25" fill="currentColor" />
            <path d="M7 4v3.4" stroke="var(--status-ink)" strokeWidth="1.6" strokeLinecap="round" />
            <circle cx="7" cy="9.9" r="0.9" fill="var(--status-ink)" />
          </>
        )}
        {status === "stopped" && (
          <>
            <circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeDasharray="2.2 2.1" />
            <path d="M5.8 5.2v3.6M8.2 5.2v3.6" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
          </>
        )}
        {status === "merged" && (
          <g fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round">
            <circle cx="4" cy="3" r="1.4" />
            <circle cx="4" cy="11" r="1.4" />
            <circle cx="10.5" cy="7.5" r="1.4" />
            <path d="M4 4.4v5.2M4 4.6c0 2.2 1.8 2.9 5.1 2.9" />
          </g>
        )}
      </svg>
    </span>
  );
}

/** Status glyph plus the state in words, for headers. */
export function StateBadge({ task }: { task: TaskDto; compact?: boolean }) {
  const status = taskStatus(task);
  const words =
    status === "merged"
      ? `Merged into ${task.merged_into}`
      : status === "attention"
        ? "Needs you"
        : stateLabel(task);
  return (
    <span className={`state-badge state-badge--${status}`}>
      <StatusIcon status={status} decorative />
      {words}
    </span>
  );
}

/** The agent's name as quiet secondary text. */
export function AgentChip({ agent }: { agent: TaskAgent }) {
  return <span className="agent-name">{AGENT_LABELS[agent] ?? agent}</span>;
}
