import { useCallback, useEffect, useRef, useState } from "react";

import {
  getTask,
  listDecisions,
  listTasks,
  onTasksChanged,
  taskActivity,
  taskLoadWarning,
  type ActivityDto,
  type DecisionDto,
  type TaskDto,
  type UnlistenFn,
} from "../../ipc";
import { errorText } from "./state";

/**
 * The backend pushes `tasks_changed`, so the list only needs a slow safety poll
 * in case the subscription never arrives or an event is missed.
 */
export const SLOW_POLL_MS = 15_000;
export const ACTIVITY_POLL_MS = 1500;

/** Subscribe to `tasks_changed` for the lifetime of a component. */
function useTasksChanged(onIds: (ids: string[]) => void) {
  const handlerRef = useRef(onIds);
  handlerRef.current = onIds;
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | null = null;
    onTasksChanged((ids) => handlerRef.current(ids))
      .then((fn) => {
        if (cancelled) void fn();
        else unlisten = fn;
      })
      .catch(() => {
        // Without the event stream the safety poll is the only refresh.
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
}

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

function sameStrings(a: string[] | undefined, b: string[] | undefined): boolean {
  const x = a ?? [];
  const y = b ?? [];
  return x.length === y.length && x.every((s, i) => s === y[i]);
}

function sameTask(a: TaskDto, b: TaskDto): boolean {
  return (
    a.state === b.state &&
    a.auto_handoff === b.auto_handoff &&
    sameStrings(a.claims, b.claims) &&
    sameOverlaps(a.claim_conflicts, b.claim_conflicts) &&
    a.title === b.title &&
    a.branch === b.branch &&
    a.exit_code === b.exit_code &&
    a.commits_ahead === b.commits_ahead &&
    a.last_activity === b.last_activity &&
    a.worktree_path === b.worktree_path &&
    a.agent === b.agent &&
    a.permission === b.permission &&
    a.model === b.model &&
    a.effort === b.effort &&
    a.runs === b.runs &&
    a.uncommitted_count === b.uncommitted_count &&
    a.attention === b.attention &&
    a.merged_into === b.merged_into &&
    a.repo_branch === b.repo_branch &&
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
 * Live task list. The backend pushes `tasks_changed`, so a change refreshes just
 * the named tasks; a full list re-read is the fallback (unknown id, an empty id
 * meaning the list itself changed, or a failed single refresh) and also runs on
 * a slow safety poll. Requests never overlap: a refresh during one in flight
 * asks for one more pass instead of racing it.
 */
export function useTasks(): TasksHandle {
  const [tasks, setTasks] = useState<TaskDto[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [loadWarning, setLoadWarning] = useState<string | null>(null);

  const mountedRef = useRef(true);
  const inFlightRef = useRef(false);
  const againRef = useRef(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const knownIdsRef = useRef<Set<string> | null>(null);

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
          knownIdsRef.current = new Set(next.map((task) => task.id));
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
    timerRef.current = setTimeout(() => void refresh(), SLOW_POLL_MS);
  }, []);

  /** Apply a `tasks_changed` event by re-reading only the tasks it names. */
  const refreshIds = useCallback(
    async (ids: string[]) => {
      const known = knownIdsRef.current;
      // An empty id, an id we have never seen, or no list yet: re-read everything.
      if (known === null || ids.length === 0 || ids.some((id) => id === "" || !known.has(id))) {
        await refresh();
        return;
      }
      // A full pass is already running (or queued); it will cover these ids.
      if (inFlightRef.current) {
        againRef.current = true;
        return;
      }
      inFlightRef.current = true;
      let failed = false;
      try {
        const updated = await Promise.all(
          ids.map((id) => getTask(id).catch(() => ((failed = true), null))),
        );
        if (!mountedRef.current) return;
        const byId = new Map(updated.filter((t): t is TaskDto => t !== null).map((t) => [t.id, t]));
        if (byId.size > 0) {
          setTasks((previous) =>
            previous
              ? reconcile(
                  previous,
                  previous.map((t) => {
                    const fresh = byId.get(t.id);
                    // get_task cannot see other tasks, so it returns no overlaps or
                    // claim conflicts; keep the ones from the last full list.
                    return fresh
                      ? {
                          ...fresh,
                          overlaps: fresh.overlaps ?? t.overlaps,
                          claim_conflicts: fresh.claim_conflicts ?? t.claim_conflicts,
                        }
                      : t;
                  }),
                )
              : previous,
          );
          setListError(null);
        }
      } finally {
        inFlightRef.current = false;
      }
      if (failed || againRef.current) {
        againRef.current = false;
        await refresh();
      }
    },
    [refresh],
  );

  useTasksChanged((ids) => void refreshIds(ids));

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
 * A task's activity timeline. Loads once, re-loads whenever `tasks_changed`
 * names this task, and keeps polling (without overlap) while it is running.
 */
export function useActivity(taskId: string | null, running: boolean) {
  const [state, setState] = useState<{
    taskId: string | null;
    items: ActivityDto[] | null;
    error: string | null;
  }>({ taskId: null, items: null, error: null });
  // Bumped by an event for this task, which re-runs the loader below.
  const [changeToken, setChangeToken] = useState(0);

  useTasksChanged((ids) => {
    if (taskId !== null && ids.some((id) => id === "" || id === taskId)) setChangeToken((n) => n + 1);
  });

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
  }, [taskId, running, changeToken]);

  const current = state.taskId === taskId;
  return { items: current ? state.items : null, error: current ? state.error : null };
}

/** How many decision-log entries the History view asks for. */
export const DECISION_LIMIT = 200;

export type DecisionsHandle = { items: DecisionDto[] | null; error: string | null };

/**
 * The repository's shared decision log. Loads while `active` and re-loads on
 * any `tasks_changed` event, since decisions are recorded as tasks move.
 */
export function useDecisions(repoPath: string | null, active: boolean): DecisionsHandle {
  const [state, setState] = useState<DecisionsHandle>({ items: null, error: null });
  const [changeToken, setChangeToken] = useState(0);

  useTasksChanged(() => {
    if (active) setChangeToken((n) => n + 1);
  });

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    listDecisions(repoPath, DECISION_LIMIT)
      .then((items) => {
        if (!cancelled) setState({ items, error: null });
      })
      .catch((e) => {
        if (!cancelled) setState((prev) => ({ items: prev.items, error: errorText(e) }));
      });
    return () => {
      cancelled = true;
    };
  }, [repoPath, active, changeToken]);

  // A repository change invalidates what is on screen straight away.
  useEffect(() => setState({ items: null, error: null }), [repoPath]);

  return state;
}
