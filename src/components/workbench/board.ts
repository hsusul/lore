// How the overview sorts tasks: what needs the user, what is live, what can land.

import type { TaskDto } from "../../ipc";

export type BoardGroup = {
  id: "attention" | "running" | "ready" | "idle" | "merged";
  title: string;
  tasks: TaskDto[];
};

/** Whether a task is waiting on the user: a usage limit, sign-in, a failed run, or a claim clash. */
export function needsUser(task: TaskDto): boolean {
  if (task.merged_into) return false;
  return Boolean(task.attention) || task.state === "failed" || (task.claim_conflicts?.length ?? 0) > 0;
}

/** Committed work, nothing uncommitted, agent stopped, not merged: what a merge would take. */
export function readyToMerge(task: TaskDto): boolean {
  return (
    !task.merged_into && task.state !== "running" && task.commits_ahead > 0 && (task.uncommitted_count ?? 0) === 0
  );
}

/** Sort the board: what needs the user first, then live work, then what can land. Empty groups drop out. */
export function groupTasks(tasks: TaskDto[]): BoardGroup[] {
  const groups: BoardGroup[] = [
    { id: "attention", title: "Needs you", tasks: [] },
    { id: "running", title: "Running", tasks: [] },
    { id: "ready", title: "Ready to merge", tasks: [] },
    { id: "idle", title: "Stopped or unfinished", tasks: [] },
    { id: "merged", title: "Merged", tasks: [] },
  ];
  const [attention, running, ready, idle, merged] = groups;
  for (const task of tasks) {
    if (task.merged_into) merged.tasks.push(task);
    else if (needsUser(task)) attention.tasks.push(task);
    else if (task.state === "running") running.tasks.push(task);
    else if (readyToMerge(task)) ready.tasks.push(task);
    else idle.tasks.push(task);
  }
  return groups.filter((g) => g.tasks.length > 0);
}
