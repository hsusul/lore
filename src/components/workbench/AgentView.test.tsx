import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({
  taskActivity: vi.fn(() => Promise.resolve([])),
  taskDiff: vi.fn(() => Promise.resolve({ text: "", truncated: false })),
  onTasksChanged: vi.fn(() => Promise.resolve(() => {})),
}));

import { taskActivity, taskDiff, type ContinueTaskRequest, type MergeResultDto, type TaskDto } from "../../ipc";
import AgentView, { usageLimitHandoffPrompt } from "./AgentView";
import { task } from "./testTask";

const handlers = () => ({
  onStop: vi.fn(() => Promise.resolve()),
  onDiscard: vi.fn(() => Promise.resolve()),
  onReveal: vi.fn(() => Promise.resolve()),
  onContinue: vi.fn<(request: ContinueTaskRequest) => Promise<void>>(() => Promise.resolve()),
  onCommit: vi.fn<(id: string, message: string, force?: boolean) => Promise<void>>(() => Promise.resolve()),
  onMerge: vi.fn<(id: string) => Promise<MergeResultDto>>(() =>
    Promise.resolve({ merged: true, into_branch: "main", conflicts: [], message: "Merged lore/fix-parser into main." }),
  ),
  onSelectTask: vi.fn(),
});

function renderView(overrides: Partial<TaskDto>, h = handlers()) {
  render(<AgentView taskId="t1" task={task({ state: "finished", ...overrides })} {...h} />);
  return h;
}

beforeEach(() => {
  vi.mocked(taskActivity).mockReset();
  vi.mocked(taskActivity).mockResolvedValue([]);
});

describe("AgentView", () => {
  it("renders the header and each activity kind distinctly", async () => {
    vi.mocked(taskActivity).mockResolvedValue([
      { kind: "message", text: "I will fix the parser." },
      { kind: "tool", text: "Edit src/a.rs" },
      { kind: "output", text: "cargo test ... ok" },
      { kind: "error", text: "permission denied" },
      { kind: "result", text: "Done: parser fixed." },
    ]);
    const h = handlers();
    render(<AgentView taskId="t1" task={task({ state: "finished", runs: 3 })} {...h} />);

    expect(screen.getByRole("heading", { name: "Fix parser" })).toBeTruthy();
    expect(screen.getByText("Finished")).toBeTruthy();
    expect(screen.getByText("Run 3")).toBeTruthy();
    // The branch carries the worktree path as its tooltip; Reveal opens it.
    expect(screen.getByText("lore/fix-parser").getAttribute("title")).toBe("/lore/worktrees/fix-parser");
    expect(screen.queryByText("/lore/worktrees/fix-parser")).toBeNull();

    const list = await screen.findByRole("list", { name: "Activity of Fix parser" });
    const items = Array.from(list.querySelectorAll("li"));
    expect(items.map((li) => li.className.replace("activity__item activity__item--", ""))).toEqual([
      "message",
      "tool",
      "output",
      "error",
      "result",
    ]);
    expect(items[1].textContent).toBe("Tool: Edit src/a.rs");
    expect(screen.queryByRole("button", { name: /Stop/ })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Reveal in Finder" }));
    await waitFor(() => expect(h.onReveal).toHaveBeenCalledWith("t1"));
    fireEvent.click(screen.getByRole("button", { name: /Discard/ }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm discard" }));
    await waitFor(() => expect(h.onDiscard).toHaveBeenCalledWith("t1"));
  });

  it("explains when the task no longer exists", () => {
    render(<AgentView taskId="gone" task={undefined} {...handlers()} />);
    expect(screen.getByText("This agent no longer exists.")).toBeTruthy();
  });

  it("shows the attention banner and an overlap warning whose task link selects it", () => {
    const h = renderView({
      attention: "Usage limit reached for Claude Code.",
      overlaps: [{ task_id: "t2", title: "Cart copy", files: ["src/a.rs", "src/c.rs"] }],
    });
    const attention = screen.getByRole("group", { name: "Needs attention" });
    expect(within(attention).getByRole("alert").textContent).toBe("Usage limit reached for Claude Code.");
    const overlap = screen.getByRole("region", { name: "Overlapping tasks" });
    expect(within(overlap).getByText("src/a.rs, src/c.rs")).toBeTruthy();
    fireEvent.click(within(overlap).getByRole("button", { name: "Cart copy" }));
    expect(h.onSelectTask).toHaveBeenCalledWith("t2");
  });

  it("continues with the same agent, or hands off when another agent is chosen (⌘Enter submits)", async () => {
    const h = renderView({});
    const input = screen.getByLabelText("Follow-up prompt");
    expect(screen.getByLabelText("Next agent").textContent).toContain("Claude Code");
    fireEvent.change(input, { target: { value: " add tests " } });
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    await waitFor(() => expect(h.onContinue).toHaveBeenCalledWith({ id: "t1", prompt: "add tests", model: "" }));
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe(""));

    fireEvent.click(screen.getByLabelText("Next agent"));
    fireEvent.click(screen.getByRole("menuitem", { name: /^Agent\b/ }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Codex" }));
    expect(screen.getByRole("menuitem", { name: /^Agent\b/ }).textContent).toContain("Codex");
    fireEvent.click(screen.getByLabelText("Next agent"));
    expect(screen.queryByRole("menu")).toBeNull();
    expect(screen.getByRole("button", { name: "Hand off to Codex" })).toBeTruthy();
    fireEvent.change(input, { target: { value: "take over" } });
    fireEvent.click(screen.getByRole("button", { name: "Hand off to Codex" }));
    await waitFor(() =>
      expect(h.onContinue).toHaveBeenLastCalledWith({
        id: "t1",
        prompt: "take over",
        agent: "codex",
        model: "",
      }),
    );
  });

  it("hides the composer, commit, and merge while running or once merged", () => {
    renderView({ state: "running", uncommitted_count: 2 });
    expect(screen.queryByRole("form", { name: "Continue task" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Commit" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Merge into/ })).toBeNull();
  });

  it("shows a merged state without continue, commit, or merge", () => {
    renderView({ merged_into: "main", uncommitted_count: 1 });
    expect(screen.getByText("Merged into main")).toBeTruthy();
    expect(screen.queryByRole("form", { name: "Continue task" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Commit" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Merge into/ })).toBeNull();
  });

  it("commits uncommitted changes with the typed message and disables while in flight", async () => {
    let finish: () => void = () => {};
    const h = handlers();
    h.onCommit.mockImplementation(() => new Promise<void>((resolve) => (finish = resolve)));
    renderView({ uncommitted_count: 2 }, h);
    expect(screen.queryByRole("button", { name: /Merge into/ })).toBeNull();
    const message = screen.getByLabelText("Commit message") as HTMLInputElement;
    expect(message.placeholder).toBe("Fix parser");
    fireEvent.change(message, { target: { value: "parser: handle eof" } });
    fireEvent.click(screen.getByRole("button", { name: "Commit" }));
    expect(h.onCommit).toHaveBeenCalledWith("t1", "parser: handle eof", undefined);
    expect(screen.getByRole("button", { name: "Committing…" }).matches(":disabled")).toBe(true);
    finish();
    await waitFor(() => expect(message.value).toBe(""));
  });

  it("warns about claim conflicts separately from overlaps and links the owning task", () => {
    const h = renderView({
      claims: ["src/parser/"],
      overlaps: [{ task_id: "t3", title: "Cart copy", files: ["src/a.rs"] }],
      claim_conflicts: [{ task_id: "t2", title: "Money owner", files: ["src/lib/money.ts", "src/b.ts"] }],
    });
    // The task's own claims show as chips in the header.
    expect(within(screen.getByRole("list", { name: "Owned paths" })).getByText("src/parser/")).toBeTruthy();

    const conflict = screen.getByRole("alert", { name: "Claim conflicts" });
    expect(conflict.textContent).toContain("Changed files owned by Money owner:");
    expect(within(conflict).getByText("src/lib/money.ts, src/b.ts")).toBeTruthy();
    // Still a distinct region from the plain overlap warning.
    expect(screen.getByRole("region", { name: "Overlapping tasks" })).toBeTruthy();

    fireEvent.click(within(conflict).getByRole("button", { name: "Money owner" }));
    expect(h.onSelectTask).toHaveBeenCalledWith("t2");
  });

  it("offers a confirmed force commit after an ownership refusal", async () => {
    const h = handlers();
    h.onCommit.mockRejectedValueOnce(new Error("this task changed files another agent owns: src/b.ts"));
    renderView({ uncommitted_count: 1, claim_conflicts: [{ task_id: "t2", title: "Owner", files: ["src/b.ts"] }] }, h);

    fireEvent.click(screen.getByRole("button", { name: "Commit" }));
    expect(await screen.findByText(/another agent owns: src\/b\.ts/)).toBeTruthy();
    expect(h.onCommit).toHaveBeenLastCalledWith("t1", "", undefined);

    // The force path needs its own confirmation before it re-sends.
    fireEvent.click(screen.getByRole("button", { name: "Commit anyway" }));
    expect(h.onCommit).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Confirm commit" }));
    await waitFor(() => expect(h.onCommit).toHaveBeenLastCalledWith("t1", "", true));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Commit anyway" })).toBeNull());
  });

  it("does not offer a force commit for an unrelated commit error", async () => {
    const h = handlers();
    h.onCommit.mockRejectedValue(new Error("nothing to commit"));
    renderView({ uncommitted_count: 1 }, h);
    fireEvent.click(screen.getByRole("button", { name: "Commit" }));
    expect(await screen.findByText("nothing to commit")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Commit anyway" })).toBeNull();
  });

  it("merges only after confirmation and shows the success message", async () => {
    const h = renderView({});
    fireEvent.click(screen.getByRole("button", { name: "Merge into checked-out branch" }));
    expect(h.onMerge).not.toHaveBeenCalled();
    expect(screen.getByText(/into your checked-out branch\?/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Confirm merge" }));
    expect(h.onMerge).toHaveBeenCalledWith("t1");
    expect((await screen.findByText("Merged lore/fix-parser into main.")).getAttribute("role")).toBe("status");
    expect(screen.queryByRole("button", { name: "Merge into checked-out branch" })).toBeNull();
  });

  it("lists conflicts and says nothing changed when the merge is aborted", async () => {
    const h = handlers();
    h.onMerge.mockResolvedValue({
      merged: false,
      into_branch: "main",
      conflicts: ["src/a.rs", "src/b.rs"],
      message: "conflict",
    });
    renderView({}, h);
    fireEvent.click(screen.getByRole("button", { name: "Merge into checked-out branch" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm merge" }));
    const files = await screen.findByRole("list", { name: "Conflicting files" });
    expect(within(files).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["src/a.rs", "src/b.rs"]);
    expect(screen.getByText(/Nothing was changed in your checkout/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Merge into checked-out branch" })).toBeTruthy();
  });

  it("disables commit, merge, and continue with a reason while the repository's merge queue runs", async () => {
    const reason = "Commit, merge, and continue are paused while this repository’s merge queue runs.";
    const h = handlers();
    const view = render(
      <AgentView taskId="t1" task={task({ state: "finished", uncommitted_count: 1 })} mergeQueueRunning {...h} />,
    );
    await screen.findByText("No activity yet.");
    expect(screen.getByText(reason).getAttribute("role")).toBe("status");
    expect(screen.getByRole("button", { name: "Commit" }).matches(":disabled")).toBe(true);
    fireEvent.submit(screen.getByRole("form", { name: "Commit changes" }));
    expect(h.onCommit).not.toHaveBeenCalled();

    // Continue stays disabled, and ⌘Enter does not send either.
    expect(screen.getByRole("button", { name: "Continue" }).matches(":disabled")).toBe(true);
    expect(screen.getByText("Paused while the merge queue runs")).toBeTruthy();
    const input = screen.getByLabelText("Follow-up prompt");
    fireEvent.change(input, { target: { value: "more" } });
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    await act(async () => {});
    expect(h.onContinue).not.toHaveBeenCalled();
    view.unmount();

    // A committed task: the merge button is disabled too.
    render(<AgentView taskId="t1" task={task({ state: "finished", repo_branch: "main" })} mergeQueueRunning {...h} />);
    await screen.findByText("No activity yet.");
    expect(screen.getByRole("button", { name: "Merge into main" }).matches(":disabled")).toBe(true);
    expect(screen.getByText(reason)).toBeTruthy();
  });

  it("does not show the queue reason for a running task, or when no queue runs", async () => {
    const view = render(<AgentView taskId="t1" task={task({ state: "running" })} mergeQueueRunning {...handlers()} />);
    await screen.findByText("No activity yet.");
    expect(screen.queryByText(/paused while this repository/)).toBeNull();
    view.unmount();
    renderView({ repo_branch: "main" });
    await screen.findByText("No activity yet.");
    expect(screen.queryByText(/paused while this repository/)).toBeNull();
    expect(screen.getByRole("button", { name: "Merge into main" }).matches(":disabled")).toBe(false);
    expect(screen.getByRole("button", { name: "Continue" }).matches(":disabled")).toBe(false);
  });

  it("shows the backend error when the merge is refused", async () => {
    const h = handlers();
    h.onMerge.mockRejectedValue("your checkout has uncommitted changes");
    renderView({}, h);
    fireEvent.click(screen.getByRole("button", { name: "Merge into checked-out branch" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm merge" }));
    expect((await screen.findByRole("alert")).textContent).toBe("your checkout has uncommitted changes");
  });

  it("hands off a usage-limited task in one click, with the orchestrator's own brief", async () => {
    const h = renderView({ state: "failed", attention: "Usage limit reached for Claude Code.", model: "opus" });
    const attention = screen.getByRole("group", { name: "Needs attention" });
    fireEvent.click(within(attention).getByRole("button", { name: "Hand off to Codex" }));
    await waitFor(() =>
      expect(h.onContinue).toHaveBeenCalledWith({
        id: "t1",
        prompt: usageLimitHandoffPrompt("Fix parser"),
        agent: "codex",
        model: "",
      }),
    );
  });

  it("briefs the other agent from a callout by setting up the composer", () => {
    renderView({ state: "failed", attention: "Not logged in to Claude Code." });
    const attention = screen.getByRole("group", { name: "Needs attention" });
    // Only a usage limit gets the one-click handoff.
    expect(within(attention).queryByRole("button", { name: /^Hand off/ })).toBeNull();
    fireEvent.click(within(attention).getByRole("button", { name: "Brief Codex…" }));
    expect(screen.getByLabelText("Next agent").textContent).toContain("Codex");
    expect(document.activeElement).toBe(screen.getByLabelText("Follow-up prompt"));
    expect(screen.getByRole("button", { name: "Hand off to Codex" })).toBeTruthy();
  });

  it("explains a failed run and offers to continue it", () => {
    renderView({ state: "failed", exit_code: 2 });
    const failed = screen.getByRole("group", { name: "Run failed" });
    expect(failed.textContent).toContain("exited with code 2");
    fireEvent.click(within(failed).getByRole("button", { name: "Continue with Claude…" }));
    expect(document.activeElement).toBe(screen.getByLabelText("Follow-up prompt"));
  });

  it("shows the task's changes in a tab beside its activity, next to commit and merge", async () => {
    vi.mocked(taskDiff).mockResolvedValue({
      text: "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\n",
      truncated: false,
    });
    renderView({ commits_ahead: 1, uncommitted_count: 0 });
    const tabs = screen.getByRole("tablist", { name: "Agent views" });
    const changes = within(tabs).getByRole("tab", { name: /Changes/ });
    expect(changes.textContent).toBe("Changes2");
    fireEvent.click(changes);
    expect(changes.getAttribute("aria-selected")).toBe("true");
    await screen.findByRole("region", { name: "src/a.rs" });
    expect(taskDiff).toHaveBeenCalledWith("t1");
    // Git actions stay in the header while reviewing.
    expect(screen.getByRole("button", { name: "Merge into checked-out branch" })).toBeTruthy();
    fireEvent.keyDown(changes, { key: "ArrowLeft" });
    expect(within(tabs).getByRole("tab", { name: "Activity" }).getAttribute("aria-selected")).toBe("true");
  });
});
