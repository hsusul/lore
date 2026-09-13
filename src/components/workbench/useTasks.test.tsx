import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({
  listTasks: vi.fn(),
  getTask: vi.fn(),
  listDecisions: vi.fn(),
  onTasksChanged: vi.fn(),
  taskLoadWarning: vi.fn(() => Promise.resolve(null)),
  taskActivity: vi.fn(),
}));

import { getTask, listDecisions, listTasks, onTasksChanged, taskActivity, type TaskDto } from "../../ipc";
import { task } from "./testTask";
import {
  ACTIVITY_POLL_MS,
  DECISION_LIMIT,
  SLOW_POLL_MS,
  useActivity,
  useDecisions,
  useTasks,
} from "./useTasks";

/** Captured `tasks_changed` subscribers, so a test can push an event. */
let subscribers: ((ids: string[]) => void)[] = [];
const unlisten = vi.fn();

function emit(ids: string[]) {
  for (const fn of [...subscribers]) fn(ids);
}

beforeEach(() => {
  subscribers = [];
  vi.mocked(onTasksChanged).mockImplementation((cb) => {
    subscribers.push(cb);
    return Promise.resolve(unlisten);
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.mocked(listTasks).mockReset();
  vi.mocked(getTask).mockReset();
  vi.mocked(listDecisions).mockReset();
  vi.mocked(taskActivity).mockReset();
  unlisten.mockReset();
});

describe("useTasks", () => {
  it("never overlaps polls and falls back to the slow safety poll", async () => {
    vi.useFakeTimers();
    let resolveFirst: (tasks: TaskDto[]) => void = () => {};
    vi.mocked(listTasks)
      .mockImplementationOnce(() => new Promise((resolve) => (resolveFirst = resolve)))
      .mockResolvedValue([task({ state: "finished" })]);
    const { result } = renderHook(() => useTasks());
    expect(listTasks).toHaveBeenCalledTimes(1);

    // The first request is still pending: neither timers nor refresh() start another.
    await act(() => vi.advanceTimersByTimeAsync(SLOW_POLL_MS * 2));
    void result.current.refresh();
    expect(listTasks).toHaveBeenCalledTimes(1);

    // Resolving runs the one queued pass; nothing ran concurrently.
    await act(async () => resolveFirst([task({})]));
    expect(listTasks).toHaveBeenCalledTimes(2);

    // No events arrive, so only the safety poll refreshes.
    await act(() => vi.advanceTimersByTimeAsync(SLOW_POLL_MS - 1));
    expect(listTasks).toHaveBeenCalledTimes(2);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(listTasks).toHaveBeenCalledTimes(3);
  });

  it("refreshes only the ids a tasks_changed event names", async () => {
    vi.mocked(listTasks).mockResolvedValue([task({}), task({ id: "t2", title: "Other" })]);
    vi.mocked(getTask).mockResolvedValue(task({ id: "t2", title: "Other", state: "finished" }));
    const { result } = renderHook(() => useTasks());
    await act(async () => {});
    expect(result.current.tasks).toHaveLength(2);

    await act(async () => emit(["t2"]));
    expect(getTask).toHaveBeenCalledTimes(1);
    expect(getTask).toHaveBeenCalledWith("t2");
    expect(listTasks).toHaveBeenCalledTimes(1); // no full re-read
    expect(result.current.tasks?.map((t) => t.state)).toEqual(["running", "finished"]);
  });

  it("falls back to list_tasks for an empty or unknown id", async () => {
    vi.mocked(listTasks).mockResolvedValue([task({})]);
    renderHook(() => useTasks());
    await act(async () => {});
    expect(listTasks).toHaveBeenCalledTimes(1);

    await act(async () => emit([""])); // the list itself changed
    expect(listTasks).toHaveBeenCalledTimes(2);
    await act(async () => emit(["brand-new"])); // an id we have not seen
    expect(listTasks).toHaveBeenCalledTimes(3);
    expect(getTask).not.toHaveBeenCalled();
  });

  it("falls back to list_tasks when a single refresh fails", async () => {
    vi.mocked(listTasks).mockResolvedValue([task({})]);
    vi.mocked(getTask).mockRejectedValue(new Error("no such task"));
    renderHook(() => useTasks());
    await act(async () => {});

    await act(async () => emit(["t1"]));
    expect(getTask).toHaveBeenCalledWith("t1");
    expect(listTasks).toHaveBeenCalledTimes(2);
  });

  it("unsubscribes on unmount", async () => {
    vi.mocked(listTasks).mockResolvedValue([]);
    const { unmount } = renderHook(() => useTasks());
    await act(async () => {});
    unmount();
    expect(unlisten).toHaveBeenCalled();
  });
});

describe("useActivity", () => {
  it("polls while running and stops once the task is not running", async () => {
    vi.useFakeTimers();
    vi.mocked(taskActivity).mockResolvedValue([{ kind: "message", text: "hi" }]);
    const { result, rerender } = renderHook(({ running }) => useActivity("t1", running), {
      initialProps: { running: true },
    });
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(result.current.items).toEqual([{ kind: "message", text: "hi" }]);
    await act(() => vi.advanceTimersByTimeAsync(ACTIVITY_POLL_MS));
    expect(taskActivity).toHaveBeenCalledTimes(2);

    rerender({ running: false });
    await act(() => vi.advanceTimersByTimeAsync(ACTIVITY_POLL_MS * 5));
    expect(taskActivity).toHaveBeenCalledTimes(3); // one final load, no further polling
  });

  it("reloads a not-running task when an event names it, and ignores other ids", async () => {
    vi.mocked(taskActivity).mockResolvedValue([]);
    renderHook(() => useActivity("t1", false));
    await act(async () => {});
    expect(taskActivity).toHaveBeenCalledTimes(1);

    await act(async () => emit(["t2"]));
    expect(taskActivity).toHaveBeenCalledTimes(1);
    await act(async () => emit(["t1"]));
    expect(taskActivity).toHaveBeenCalledTimes(2);
  });
});

describe("useDecisions", () => {
  it("loads the open repository's log and reloads on any event", async () => {
    vi.mocked(listDecisions).mockResolvedValue([
      { at_ms: 1, task_id: "t1", task_title: "Fix parser", repo_path: "/repo", kind: "created", detail: "d" },
    ]);
    const { result } = renderHook(() => useDecisions("/repo", true));
    await act(async () => {});
    expect(listDecisions).toHaveBeenCalledWith("/repo", DECISION_LIMIT);
    expect(result.current.items).toHaveLength(1);

    await act(async () => emit(["t1"]));
    expect(listDecisions).toHaveBeenCalledTimes(2);
  });

  it("does not load while the tab is inactive", async () => {
    vi.mocked(listDecisions).mockResolvedValue([]);
    renderHook(() => useDecisions("/repo", false));
    await act(async () => {});
    await act(async () => emit(["t1"]));
    expect(listDecisions).not.toHaveBeenCalled();
  });
});
