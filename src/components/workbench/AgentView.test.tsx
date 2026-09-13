import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({
  taskActivity: vi.fn(() => Promise.resolve([])),
  onTasksChanged: vi.fn(() => Promise.resolve(() => {})),
}));

import { taskActivity, type ContinueTaskRequest, type MergeResultDto, type TaskDto } from "../../ipc";
import AgentView from "./AgentView";
import { task } from "./testTask";

const handlers = () => ({
  onStop: vi.fn(() => Promise.resolve()),
  onDiscard: vi.fn(() => Promise.resolve()),
  onReveal: vi.fn(() => Promise.resolve()),
  onOpenDiff: vi.fn(),
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
    expect(screen.getByText("lore/fix-parser")).toBeTruthy();
    expect(screen.getByText("/lore/worktrees/fix-parser")).toBeTruthy();

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
    expect(screen.getByRole("alert", { name: "Needs attention" }).textContent).toBe(
      "Usage limit reached for Claude Code.",
    );
    const overlap = screen.getByRole("region", { name: "Overlapping tasks" });
    expect(within(overlap).getByText("src/a.rs, src/c.rs")).toBeTruthy();
    fireEvent.click(within(overlap).getByRole("button", { name: "Cart copy" }));
    expect(h.onSelectTask).toHaveBeenCalledWith("t2");
  });

  it("continues with the same agent, or hands off when another agent is chosen (⌘Enter submits)", async () => {
    const h = renderView({});
    const input = screen.getByLabelText("Follow-up prompt");
    expect((screen.getByLabelText("Next agent") as HTMLSelectElement).value).toBe("claude_code");
    fireEvent.change(input, { target: { value: " add tests " } });
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    await waitFor(() => expect(h.onContinue).toHaveBeenCalledWith({ id: "t1", prompt: "add tests" }));
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe(""));

    fireEvent.change(screen.getByLabelText("Next agent"), { target: { value: "codex" } });
    expect(screen.getByRole("button", { name: "Hand off to Codex" })).toBeTruthy();
    fireEvent.change(input, { target: { value: "take over" } });
    fireEvent.click(screen.getByRole("button", { name: "Hand off to Codex" }));
    await waitFor(() =>
      expect(h.onContinue).toHaveBeenLastCalledWith({ id: "t1", prompt: "take over", agent: "codex" }),
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

  it("shows the backend error when the merge is refused", async () => {
    const h = handlers();
    h.onMerge.mockRejectedValue("your checkout has uncommitted changes");
    renderView({}, h);
    fireEvent.click(screen.getByRole("button", { name: "Merge into checked-out branch" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm merge" }));
    expect((await screen.findByRole("alert")).textContent).toBe("your checkout has uncommitted changes");
  });
});
