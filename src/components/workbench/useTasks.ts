import { useCallback, useEffect, useRef, useState } from "react";

import { listTasks, taskActivity, taskLoadWarning, type ActivityDto, type TaskDto } from "../../ipc";
import { errorText } from "./state";

/** Poll cadence while at least one task is running, and otherwise. */
export const FAST_POLL_MS = 1500;
export const SLOW_POLL_MS = 10_000;
export const ACTIVITY_POLL_MS = 1500;

function sameOverlaps(a: TaskDto["overlaps"], b: TaskDto["overlaps"]): boolean {
  const x = a ?? [];
  const y = b ?? [];
  return (
    x.length === y.length &&
    x.every(
      (o, i) =>
        o.task_id === y[i].task_id &&
        o.title === y[i].title &&
        o.files.length === y[i].files.length &&
        o.files.every((f, j) => f === y[i].files[j]),
    )
  );
}

function sameTask(a: TaskDto, b: TaskDto): boolean {
  return (
    a.state === b.state &&
    a.title === b.title &&
    a.branch === b.branch &&
    a.exit_code === b.exit_code &&
    a.commits_ahead === b.commits_ahead &&
    a.last_activity === b.last_activity &&
    a.worktree_path === b.worktree_path &&
    a.agent === b.agent &&
    a.permission === b.permission &&
    a.runs === b.runs &&
    a.uncommitted_count === b.uncommitted_count &&
    a.attention === b.attention &&
    a.merged_into === b.merged_into &&
    sameOverlaps(a.overlaps, b.overlaps) &&
    a.changed_files.length === b.changed_files.length &&
    a.changed_files.every((file, i) => file === b.changed_files[i])
  );
}

/** Keep previous object identities for unchanged tasks so memoized rows skip rendering. */
function reconcile(previous: TaskDto[], next: TaskDto[]): TaskDto[] {
  const byId = new Map(previous.map((task) => [task.id, task]));
  let changed = previous.length !== next.length;
  const merged = next.map((task, i) => {
    const old = byId.get(task.id);
    if (old && sameTask(old, task)) {
      if (previous[i] !== old) changed = true;
      return old;
    }
    changed = true;
    return task;
  });
  return changed ? merged : previous;
}

export type TasksHandle = {
  tasks: TaskDto[] | null;
  listError: string | null;
  loadWarning: string | null;
  refresh: () => Promise<void>;
};

/**
 * Live task list: polls fast while any task runs and slowly otherwise, and never
 * overlaps requests (a refresh during a poll asks for one more pass).
 */
export function useTasks(): TasksHandle {
  const [tasks, setTasks] = useState<TaskDto[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [loadWarning, setLoadWarning] = useState<string | null>(null);

  const mountedRef = useRef(true);
  const inFlightRef = useRef(false);
  const againRef = useRef(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const runningRef = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlightRef.current) {
      againRef.current = true;
      return;
    }
    inFlightRef.current = true;
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    try {
      do {
        againRef.current = false;
        try {
          const next = await listTasks();
          if (!mountedRef.current) return;
          runningRef.current = next.some((task) => task.state === "running");
          setTasks((previous) => (previous ? reconcile(previous, next) : next));
          setListError(null);
        } catch (e) {
          if (!mountedRef.current) return;
          setListError(errorText(e));
        }
      } while (againRef.current && mountedRef.current);
    } finally {
      inFlightRef.current = false;
    }
    if (!mountedRef.current) return;
    timerRef.current = setTimeout(
      () => void refresh(),
      runningRef.current ? FAST_POLL_MS : SLOW_POLL_MS,
    );
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    void refresh();
    taskLoadWarning()
      .then((warning) => {
        if (mountedRef.current) setLoadWarning(warning);
      })
      .catch(() => {});
    return () => {
      mountedRef.current = false;
      if (timerRef.current !== null) clearTimeout(timerRef.current);
    };
  }, [refresh]);

  return { tasks, listError, loadWarning, refresh };
}

function sameActivity(a: ActivityDto[], b: ActivityDto[]): boolean {
  return a.length === b.length && a.every((x, i) => x.kind === b[i].kind && x.text === b[i].text);
}

/**
 * A task's activity timeline. Loads once, and keeps polling (without overlap)
 * while the task is running.
 */
export function useActivity(taskId: string | null, running: boolean) {
  const [state, setState] = useState<{
    taskId: string | null;
    items: ActivityDto[] | null;
    error: string | null;
  }>({ taskId: null, items: null, error: null });

  useEffect(() => {
    if (taskId === null) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    async function tick() {
      try {
        const next = await taskActivity(taskId as string);
        if (cancelled) return;
        setState((prev) =>
          prev.taskId === taskId && prev.items && sameActivity(prev.items, next) && !prev.error
            ? prev
            : { taskId, items: next, error: null },
        );
      } catch (e) {
        if (cancelled) return;
        setState((prev) => ({
          taskId,
          items: prev.taskId === taskId ? prev.items : null,
          error: errorText(e),
        }));
      }
      if (!cancelled && running) timer = setTimeout(() => void tick(), ACTIVITY_POLL_MS);
    }
    void tick();
    return () => {
      cancelled = true;
      if (timer !== null) clearTimeout(timer);
    };
  }, [taskId, running]);

  const current = state.taskId === taskId;
  return { items: current ? state.items : null, error: current ? state.error : null };
}
