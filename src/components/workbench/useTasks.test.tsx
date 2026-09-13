import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({
  listTasks: vi.fn(),
  taskLoadWarning: vi.fn(() => Promise.resolve(null)),
  taskActivity: vi.fn(),
}));

import { listTasks, taskActivity, type TaskDto } from "../../ipc";
import { task } from "./testTask";
import { ACTIVITY_POLL_MS, FAST_POLL_MS, SLOW_POLL_MS, useActivity, useTasks } from "./useTasks";

afterEach(() => {
  vi.useRealTimers();
  vi.mocked(listTasks).mockReset();
  vi.mocked(taskActivity).mockReset();
});

describe("useTasks", () => {
  it("never overlaps polls and polls fast only while a task runs", async () => {
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

    // The queued pass saw nothing running, so the next poll waits the slow interval.
    await act(() => vi.advanceTimersByTimeAsync(FAST_POLL_MS * 2));
    expect(listTasks).toHaveBeenCalledTimes(2);
    await act(() => vi.advanceTimersByTimeAsync(SLOW_POLL_MS));
    expect(listTasks).toHaveBeenCalledTimes(3);
  });

  it("polls fast while a task is running", async () => {
    vi.useFakeTimers();
    vi.mocked(listTasks).mockResolvedValue([task({})]);
    const { result } = renderHook(() => useTasks());
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(result.current.tasks?.[0].id).toBe("t1");
    await act(() => vi.advanceTimersByTimeAsync(FAST_POLL_MS));
    expect(listTasks).toHaveBeenCalledTimes(2);
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
});
