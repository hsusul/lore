import type { TaskAgent, TaskDto } from "../../ipc";
import { AGENT_LABELS, stateLabel } from "./state";

/** A small status mark: a dot plus the state word, used on rows, cards, and the agent header. */
export function StateBadge({ task, compact }: { task: TaskDto; compact?: boolean }) {
  if (task.merged_into) {
    return (
      <span className="state-badge state-badge--merged">
        <span className="state-badge__dot" aria-hidden="true" />
        {compact ? "Merged" : `Merged into ${task.merged_into}`}
      </span>
    );
  }
  return (
    <span className={`state-badge state-badge--${task.state}`}>
      <span className="state-badge__dot" aria-hidden="true" />
      {compact && task.state === "failed" ? "Failed" : stateLabel(task)}
    </span>
  );
}

/** The agent's name in its own colour, so Claude Code and Codex tasks tell apart at a glance. */
export function AgentChip({ agent }: { agent: TaskAgent }) {
  return (
    <span className={`agent-chip agent-chip--${agent}`}>
      <span className="agent-chip__dot" aria-hidden="true" />
      {AGENT_LABELS[agent] ?? agent}
    </span>
  );
}
