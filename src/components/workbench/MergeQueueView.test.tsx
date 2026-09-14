import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({
  getRepoSettings: vi.fn(),
  setRepoTestCommand: vi.fn(),
  startMergeQueue: vi.fn(),
  getMergeQueue: vi.fn(),
  cancelMergeQueue: vi.fn(),
}));

import {
  cancelMergeQueue,
  getMergeQueue,
  getRepoSettings,
  setRepoTestCommand,
  startMergeQueue,
  type MergeQueueDto,
  type TaskDto,
} from "../../ipc";
import MergeQueueView from "./MergeQueueView";
import { task } from "./testTask";
import { MERGE_QUEUE_POLL_MS, useMergeQueues, type MergeQueueEntry } from "./useMergeQueue";

const READY = [
  task({ id: "a", title: "Alpha", state: "finished", repo_branch: "main", branch: "lore/alpha" }),
  task({ id: "b", title: "Beta", state: "finished", repo_branch: "main", branch: "lore/beta" }),
  task({ id: "c", title: "Gamma", state: "stopped", repo_branch: "main", branch: "lore/gamma" }),
];

type Row = [taskId: string, title: string, status: string, detail?: string | null];

function queue(running: boolean, rows: Row[], testCommand: string | null = "npm test"): MergeQueueDto {
  return {
    repo_path: "/repo",
    running,
    test_command: testCommand,
    items: rows.map(([task_id, title, status, detail = null]) => ({ task_id, title, status, detail })),
  };
}

/** The view wired to the real queue hook, as the workbench does. */
function Harness({ tasks, onFinished }: { tasks: TaskDto[]; onFinished: () => void }) {
  const queues = useMergeQueues("/repo", onFinished);
  return (
    <MergeQueueView
      repoPath="/repo"
      tasks={tasks}
      entry={queues.entries["/repo"] ?? null}
      onStart={queues.start}
      onCancel={queues.cancel}
      onDismiss={queues.dismiss}
    />
  );
}

function renderQueue(tasks: TaskDto[] = READY) {
  const onFinished = vi.fn();
  const view = render(<Harness tasks={tasks} onFinished={onFinished} />);
  return { onFinished, view };
}

async function renderEntry(entry: MergeQueueEntry) {
  const props = { onStart: vi.fn(), onCancel: vi.fn(), onDismiss: vi.fn() };
  render(<MergeQueueView repoPath="/repo" tasks={READY} entry={entry} {...props} />);
  await screen.findByDisplayValue("npm test"); // settings loaded
  return props;
}

/** Let resolved IPC promises land while timers are faked. */
const flush = () => act(() => vi.advanceTimersByTimeAsync(0));

const queueItem = (title: string) => within(screen.getByRole("list", { name: "Merge queue" })).getByRole("listitem", { name: title });

beforeEach(() => {
  vi.mocked(getRepoSettings).mockReset().mockResolvedValue({ repo_path: "/repo", test_command: "npm test" });
  vi.mocked(setRepoTestCommand)
    .mockReset()
    .mockImplementation((_repo, command) => Promise.resolve({ repo_path: "/repo", test_command: command }));
  vi.mocked(startMergeQueue).mockReset();
  vi.mocked(getMergeQueue).mockReset().mockResolvedValue(null);
  vi.mocked(cancelMergeQueue).mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  vi.useRealTimers();
});

describe("MergeQueueView setup", () => {
  it("offers only the open repository's committed, clean, stopped, unmerged tasks", async () => {
    renderQueue([
      task({ id: "ok", title: "Ready one", state: "finished" }),
      task({ id: "run", title: "Still running", state: "running" }),
      task({ id: "dirty", title: "Dirty", state: "finished", uncommitted_count: 2 }),
      task({ id: "empty", title: "No commits", state: "failed", commits_ahead: 0 }),
      task({ id: "done", title: "Merged", state: "finished", merged_into: "main" }),
      task({ id: "far", title: "Elsewhere", state: "finished", repo_path: "/other" }),
    ]);
    await screen.findByDisplayValue("npm test");
    const picks = within(screen.getByRole("list", { name: "Tasks ready to merge" }));
    expect(picks.getAllByRole("checkbox").map((box) => box.closest("label")?.textContent)).toEqual(["Ready one"]);
    // Merged and other-repository tasks are not counted as waiting.
    expect(screen.getByText(/3 other tasks aren’t ready/)).toBeTruthy();
  });

  it("explains when nothing is ready and when no folder is open", async () => {
    const { view } = renderQueue([task({ state: "running" })]);
    expect(await screen.findByText(/No tasks are ready/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Merge 0 in order" }).matches(":disabled")).toBe(true);
    expect(screen.getByRole("button", { name: "Merge 0 in order" }).closest(".merge-queue__dock")).toBeTruthy();
    view.unmount();
    render(<MergeQueueView repoPath={null} tasks={[]} entry={null} onStart={vi.fn()} onCancel={vi.fn()} onDismiss={vi.fn()} />);
    expect(screen.getByText("Open a folder to merge its tasks in order.")).toBeTruthy();
  });

  it("merges the checked tasks in the arranged order after an inline confirm", async () => {
    vi.mocked(startMergeQueue).mockResolvedValue(queue(true, [["c", "Gamma", "pending"], ["a", "Alpha", "pending"]]));
    renderQueue();
    await screen.findByDisplayValue("npm test");

    fireEvent.click(screen.getByRole("checkbox", { name: "Alpha" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Gamma" }));
    expect(screen.getByRole("button", { name: "Merge 2 in order" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Move Gamma up" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Move Gamma up" }));
    fireEvent.click(screen.getByRole("button", { name: "Move Gamma up" }));
    const titles = within(screen.getByRole("list", { name: "Tasks ready to merge" }))
      .getAllByRole("checkbox")
      .map((box) => box.closest("label")?.textContent);
    expect(titles).toEqual(["Gamma", "Alpha", "Beta"]);
    // At the top the up button is disabled, so focus stays on the task via its down button.
    expect(screen.getByRole("button", { name: "Move Gamma up" }).matches(":disabled")).toBe(true);
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Move Gamma down" }));
    expect(screen.getByText("Gamma moved to position 1 of 3.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Move Beta down" }).matches(":disabled")).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Merge 2 in order" }));
    const confirm = await screen.findByRole("group", { name: "Confirm merge queue" });
    expect(confirm.textContent).toContain("Merge 2 tasks into main, in this order?");
    expect(confirm.textContent).toContain("Tests run first: npm test");
    // Choices are frozen while confirming.
    expect(screen.getByRole("checkbox", { name: "Beta" }).matches(":disabled")).toBe(true);
    expect(startMergeQueue).not.toHaveBeenCalled();

    fireEvent.click(within(confirm).getByRole("button", { name: "Confirm merge" }));
    await waitFor(() => expect(startMergeQueue).toHaveBeenCalledWith("/repo", ["c", "a"]));
    expect(await screen.findByRole("list", { name: "Merge queue" })).toBeTruthy();
    expect(screen.queryByRole("group", { name: "Confirm merge queue" })).toBeNull();
  });

  it("says when tasks merge without testing and names the checked-out branch when unknown", async () => {
    vi.mocked(getRepoSettings).mockResolvedValue({ repo_path: "/repo", test_command: null });
    renderQueue([task({ id: "a", title: "Alpha", state: "finished" })]);
    const input = (await screen.findByPlaceholderText(
      "e.g. npm test — leave empty to merge without testing",
    )) as HTMLInputElement;
    await waitFor(() => expect(input.disabled).toBe(false));
    expect(input.value).toBe("");

    fireEvent.click(screen.getByRole("checkbox", { name: "Alpha" }));
    fireEvent.click(screen.getByRole("button", { name: "Merge 1 in order" }));
    const confirm = await screen.findByRole("group", { name: "Confirm merge queue" });
    expect(confirm.textContent).toContain("Merge 1 task into your checked-out branch, in this order?");
    expect(confirm.textContent).toContain("No test command is set, so tasks merge without testing.");
    fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("group", { name: "Confirm merge queue" })).toBeNull();
    expect(setRepoTestCommand).not.toHaveBeenCalled();
  });

  it("shows why the backend refused to start, keeping the selection", async () => {
    vi.mocked(startMergeQueue).mockRejectedValue("a merge queue is already running for this repository");
    renderQueue();
    await screen.findByDisplayValue("npm test");
    fireEvent.click(screen.getByRole("checkbox", { name: "Beta" }));
    fireEvent.click(screen.getByRole("button", { name: "Merge 1 in order" }));
    fireEvent.click(await screen.findByRole("button", { name: "Confirm merge" }));
    expect((await screen.findByRole("alert")).textContent).toBe("a merge queue is already running for this repository");
    expect((screen.getByRole("checkbox", { name: "Beta" }) as HTMLInputElement).checked).toBe(true);
    expect(screen.queryByRole("list", { name: "Merge queue" })).toBeNull();
  });
});

describe("MergeQueueView test command", () => {
  it("loads the command, saves on Enter and on blur, and shows Saved", async () => {
    renderQueue();
    const input = (await screen.findByDisplayValue("npm test")) as HTMLInputElement;
    expect(getRepoSettings).toHaveBeenCalledWith("/repo");
    expect(input.placeholder).toBe("e.g. npm test — leave empty to merge without testing");

    fireEvent.change(input, { target: { value: " cargo test " } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByText("Saved")).toBeTruthy();
    expect(setRepoTestCommand).toHaveBeenCalledWith("/repo", "cargo test");
    expect(input.value).toBe("cargo test");

    // Blurring an unchanged command does not save again.
    fireEvent.blur(input);
    await act(async () => {});
    expect(setRepoTestCommand).toHaveBeenCalledTimes(1);

    // Editing hides "Saved"; clearing saves null.
    fireEvent.change(input, { target: { value: "   " } });
    expect(screen.queryByText("Saved")).toBeNull();
    fireEvent.blur(input);
    await waitFor(() => expect(setRepoTestCommand).toHaveBeenLastCalledWith("/repo", null));
    expect(await screen.findByText("Saved")).toBeTruthy();
    expect(input.value).toBe("");
  });

  it("saves an edit still in the field before confirming, and does not confirm if saving fails", async () => {
    renderQueue();
    const input = await screen.findByDisplayValue("npm test");
    fireEvent.click(screen.getByRole("checkbox", { name: "Alpha" }));

    fireEvent.change(input, { target: { value: "make check" } });
    fireEvent.click(screen.getByRole("button", { name: "Merge 1 in order" }));
    const confirm = await screen.findByRole("group", { name: "Confirm merge queue" });
    expect(setRepoTestCommand).toHaveBeenCalledWith("/repo", "make check");
    expect(confirm.textContent).toContain("Tests run first: make check");
    fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));

    vi.mocked(setRepoTestCommand).mockRejectedValue("test command must be under 2000 characters");
    fireEvent.change(input, { target: { value: "x".repeat(2001) } });
    fireEvent.click(screen.getByRole("button", { name: "Merge 1 in order" }));
    expect((await screen.findByRole("alert")).textContent).toBe("test command must be under 2000 characters");
    expect(screen.queryByRole("group", { name: "Confirm merge queue" })).toBeNull();
  });
});

describe("MergeQueueView while running", () => {
  async function startTwo() {
    vi.useFakeTimers();
    vi.mocked(startMergeQueue).mockResolvedValue(queue(true, [["a", "Alpha", "pending"], ["b", "Beta", "pending"]]));
    const rendered = renderQueue();
    await flush();
    fireEvent.click(screen.getByRole("checkbox", { name: "Alpha" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Beta" }));
    fireEvent.click(screen.getByRole("button", { name: "Merge 2 in order" }));
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "Confirm merge" }));
    await flush();
    expect(startMergeQueue).toHaveBeenCalledWith("/repo", ["a", "b"]);
    return rendered;
  }

  it("polls every second, shows each transition, and stops once the queue is not running", async () => {
    const { onFinished } = await startTwo();
    const mountCalls = vi.mocked(getMergeQueue).mock.calls.length; // the on-open check
    expect(within(queueItem("Alpha")).getByText("Pending")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Merge 2 in order" }).matches(":disabled")).toBe(true);
    expect(screen.getByText("A queue is running for this repository.")).toBeTruthy();

    vi.mocked(getMergeQueue).mockResolvedValue(queue(true, [["a", "Alpha", "testing", "npm test"], ["b", "Beta", "pending"]]));
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS - 1));
    expect(getMergeQueue).toHaveBeenCalledTimes(mountCalls);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(getMergeQueue).toHaveBeenLastCalledWith("/repo");
    const alpha = queueItem("Alpha");
    expect(alpha.className).toContain("mq-item--active");
    expect(within(alpha).getByText("Testing")).toBeTruthy();
    expect(within(alpha).getByText("npm test")).toBeTruthy();
    expect(screen.getByText("Merge queue running: 1 of 2, testing Alpha")).toBeTruthy();

    vi.mocked(getMergeQueue).mockResolvedValue(
      queue(true, [["a", "Alpha", "merged", "Merged lore/alpha into main."], ["b", "Beta", "merging"]]),
    );
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS));
    expect(within(queueItem("Alpha")).getByText("Merged")).toBeTruthy();
    expect(within(queueItem("Beta")).getByText("Merging")).toBeTruthy();
    expect(onFinished).not.toHaveBeenCalled();

    vi.mocked(getMergeQueue).mockResolvedValue(
      queue(false, [["a", "Alpha", "merged", "Merged lore/alpha into main."], ["b", "Beta", "merged", "Merged lore/beta into main."]]),
    );
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS));
    expect(screen.getByText("Merge queue finished: 2 merged")).toBeTruthy();
    expect(onFinished).toHaveBeenCalledTimes(1); // the task list is refreshed
    const calls = vi.mocked(getMergeQueue).mock.calls.length;

    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS * 5));
    expect(getMergeQueue).toHaveBeenCalledTimes(calls);

    // Results stay until dismissed.
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("list", { name: "Merge queue" })).toBeNull();
    expect(screen.getByRole("button", { name: "Merge 2 in order" }).matches(":disabled")).toBe(false);
  });

  it("stops polling when unmounted", async () => {
    const { view } = await startTwo();
    vi.mocked(getMergeQueue).mockResolvedValue(queue(true, [["a", "Alpha", "updating"], ["b", "Beta", "pending"]]));
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS));
    const calls = vi.mocked(getMergeQueue).mock.calls.length;
    view.unmount();
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS * 5));
    expect(getMergeQueue).toHaveBeenCalledTimes(calls);
  });

  it("cancels, then shows the cancelled result", async () => {
    await startTwo();
    vi.mocked(getMergeQueue).mockResolvedValue(queue(true, [["a", "Alpha", "testing", "npm test"], ["b", "Beta", "pending"]]));
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS));

    fireEvent.click(screen.getByRole("button", { name: "Cancel queue" }));
    await flush();
    expect(cancelMergeQueue).toHaveBeenCalledWith("/repo");
    expect(screen.getByRole("button", { name: "Cancelling…" }).matches(":disabled")).toBe(true);

    // Still running for one more poll: the button stays in its cancelling state.
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS));
    expect(screen.getByRole("button", { name: "Cancelling…" })).toBeTruthy();

    vi.mocked(getMergeQueue).mockResolvedValue(
      queue(false, [["a", "Alpha", "cancelled", "tests failed: cancelled"], ["b", "Beta", "cancelled"]]),
    );
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS));
    expect(screen.getByText("Merge queue cancelled: 2 cancelled")).toBeTruthy();
    expect(queueItem("Beta").className).toContain("mq-item--muted");
    expect(within(queueItem("Alpha")).getByText("tests failed: cancelled")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Cancel/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Dismiss" })).toBeTruthy();
  });

  it("picks up a queue that is already running when the repository opens", async () => {
    vi.useFakeTimers();
    vi.mocked(getMergeQueue).mockResolvedValue(queue(true, [["a", "Alpha", "merging"]], null));
    renderQueue();
    await flush();
    expect(within(queueItem("Alpha")).getByText("Merging")).toBeTruthy();
    expect(screen.getByText("Merging without a test command")).toBeTruthy();
    vi.mocked(getMergeQueue).mockResolvedValue(queue(false, [["a", "Alpha", "merged"]], null));
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS));
    expect(screen.getByText("Merge queue finished: 1 merged")).toBeTruthy();
  });

  it("keeps polling through a failed read and shows the error", async () => {
    await startTwo();
    vi.mocked(getMergeQueue).mockRejectedValueOnce("orchestrator is busy");
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS));
    expect(screen.getByRole("alert").textContent).toBe("orchestrator is busy");
    vi.mocked(getMergeQueue).mockResolvedValue(queue(true, [["a", "Alpha", "updating"], ["b", "Beta", "pending"]]));
    await act(() => vi.advanceTimersByTimeAsync(MERGE_QUEUE_POLL_MS));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(within(queueItem("Alpha")).getByText("Updating")).toBeTruthy();
  });
});

describe("MergeQueueView results", () => {
  it("shows a failure in red with its output in an expandable monospace block, and later tasks skipped", async () => {
    const output = "tests failed: exit 1; last output:\n FAIL  src/a.test.ts > adds\nAssertionError: expected 3, got 2";
    const props = await renderEntry({
      queue: queue(false, [
        ["a", "Alpha", "merged", "Merged lore/alpha into main (1 commit)."],
        ["b", "Beta", "failed", output],
        ["c", "Gamma", "skipped", "an earlier task failed"],
      ]),
      cancelling: false,
      error: null,
    });
    expect(screen.getByText("Merge queue stopped: 1 merged, 1 failed, 1 skipped")).toBeTruthy();

    const beta = queueItem("Beta");
    expect(beta.className).toContain("mq-item--failed");
    expect(within(beta).getByText("Failed")).toBeTruthy();
    const details = beta.querySelector("details");
    expect(details).not.toBeNull();
    expect(details?.open).toBe(false);
    expect(details?.querySelector("summary")?.textContent).toBe("tests failed: exit 1; last output:");
    const pre = details?.querySelector("pre");
    expect(pre?.className).toBe("mq-item__output");
    expect(pre?.textContent).toBe(output);

    const gamma = queueItem("Gamma");
    expect(gamma.className).toContain("mq-item--muted");
    expect(within(gamma).getByText("Skipped")).toBeTruthy();
    expect(within(gamma).getByText("an earlier task failed")).toBeTruthy();
    expect(within(queueItem("Alpha")).getByText("Merged lore/alpha into main (1 commit).")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(props.onDismiss).toHaveBeenCalledWith("/repo");
  });

  it("renders an unknown status from a newer backend as text", async () => {
    await renderEntry({ queue: queue(true, [["a", "Alpha", "rebasing"]]), cancelling: false, error: null });
    expect(within(queueItem("Alpha")).getByText("rebasing")).toBeTruthy();
  });
});
