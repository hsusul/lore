import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({ createTask: vi.fn() }));

import { usageLimitHandoffPrompt } from "./AgentView";
import BoardView, { ViewSwitch } from "./BoardView";
import { task } from "./testTask";

const BOARD = [
  task({ id: "run", title: "Running one", repo_path: "/work/acme", repo_branch: "main" }),
  task({ id: "limit", title: "Limited", state: "failed", attention: "Usage limit reached. Resets at 3:00 PM." }),
  task({ id: "crash", title: "Crashed", state: "failed", exit_code: 1 }),
  task({ id: "ready", title: "Ready one", state: "finished", commits_ahead: 2, uncommitted_count: 0 }),
  task({ id: "dirty", title: "Dirty one", state: "finished", commits_ahead: 0, uncommitted_count: 3 }),
  task({ id: "done", title: "Landed", state: "finished", merged_into: "main" }),
];

function renderBoard(overrides: Partial<Parameters<typeof BoardView>[0]> = {}) {
  const props = {
    workspace: "/work/acme",
    tasks: BOARD,
    onSelect: vi.fn(),
    onOpenDiff: vi.fn(),
    onContinue: vi.fn(() => Promise.resolve()),
    onOpenMergeQueue: vi.fn(),
    onOpenFolder: vi.fn(),
    onCreated: vi.fn(),
    queueRunning: vi.fn(() => false),
    ...overrides,
  };
  render(<BoardView {...props} />);
  return props;
}

function cardNames(column: string) {
  return within(screen.getByRole("region", { name: column }))
    .queryAllByRole("button")
    .filter((b) => b.classList.contains("board-card__main"))
    .map((b) => b.getAttribute("aria-label"));
}

describe("BoardView", () => {
  it("puts every agent in a column by what it needs, under the repository and its branch", () => {
    renderBoard();
    const overview = screen.getByRole("region", { name: "Overview" });
    expect(within(overview).getByRole("heading", { name: "acme on main" })).toBeTruthy();
    expect(screen.getByText("2 need you · 1 running · 1 ready to merge")).toBeTruthy();
    expect(cardNames("Needs you")).toEqual(["Limited", "Crashed"]);
    expect(cardNames("Running")).toEqual(["Running one"]);
    expect(cardNames("Ready to merge")).toEqual(["Ready one"]);
    expect(cardNames("To review")).toEqual(["Dirty one"]);
    expect(cardNames("Merged")).toEqual(["Landed"]);

    // Each card says what needs you, what the agent is doing, or what is left.
    const needs = screen.getByRole("region", { name: "Needs you" });
    expect(within(needs).getByText("Usage limit reached. Resets at 3:00 PM.")).toBeTruthy();
    expect(within(needs).getByText("Failed (exit 1)")).toBeTruthy();
    expect(within(screen.getByRole("region", { name: "Running" })).getByText("Editing src/a.rs")).toBeTruthy();
    expect(within(screen.getByRole("region", { name: "To review" })).getByText("3 uncommitted changes")).toBeTruthy();
    expect(within(screen.getByRole("region", { name: "Ready to merge" })).getByText("2 commits")).toBeTruthy();
  });

  it("keeps its main columns when they are empty, and drops the others", () => {
    renderBoard({ tasks: [] });
    expect(screen.getByText("Nothing needs you.")).toBeTruthy();
    expect(screen.getByText("No agents running.")).toBeTruthy();
    expect(screen.getByText("Nothing to merge yet.")).toBeTruthy();
    expect(screen.queryByRole("region", { name: "To review" })).toBeNull();
    expect(screen.queryByRole("region", { name: "Merged" })).toBeNull();
    expect(screen.getByText("No agents yet")).toBeTruthy();
  });

  it("opens an agent from its card", () => {
    const props = renderBoard();
    fireEvent.click(screen.getByRole("button", { name: "Ready one" }));
    expect(props.onSelect).toHaveBeenCalledWith("ready");
  });

  it("hands a usage-limited agent to the other agent from its card, unless a merge queue holds the repository", async () => {
    const props = renderBoard();
    const needs = screen.getByRole("region", { name: "Needs you" });
    // Only the usage limit offers a handoff; a crash is opened and read first.
    expect(within(needs).getAllByRole("button", { name: /Hand off/ })).toHaveLength(1);
    fireEvent.click(within(needs).getByRole("button", { name: "Hand off to Codex" }));
    await waitFor(() =>
      expect(props.onContinue).toHaveBeenCalledWith({
        id: "limit",
        prompt: usageLimitHandoffPrompt("Limited"),
        agent: "codex",
        model: "",
      }),
    );

    renderBoard({ queueRunning: vi.fn(() => true) });
    const again = screen.getAllByRole("region", { name: "Needs you" })[1];
    expect(within(again).getByRole("button", { name: "Hand off to Codex" }).hasAttribute("disabled")).toBe(true);
  });

  it("shows why a handoff failed on the card", async () => {
    renderBoard({ onContinue: vi.fn(() => Promise.reject(new Error("Queue is busy"))) });
    fireEvent.click(screen.getByRole("button", { name: "Hand off to Codex" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Queue is busy");
  });

  it("reviews a claim clash, warns about overlaps, and links ready work to the merge queue", () => {
    const props = renderBoard({
      tasks: [
        task({
          id: "clash",
          title: "Clash",
          state: "finished",
          claim_conflicts: [{ task_id: "ready", title: "Ready one", files: ["src/a.rs"] }],
        }),
        task({
          id: "ready",
          title: "Ready one",
          state: "finished",
          uncommitted_count: 0,
          overlaps: [{ task_id: "clash", title: "Clash", files: ["src/a.rs"] }],
        }),
      ],
    });
    const needs = screen.getByRole("region", { name: "Needs you" });
    expect(within(needs).getByText("Changed files another agent owns")).toBeTruthy();
    fireEvent.click(within(needs).getByRole("button", { name: "Review changes" }));
    expect(props.onOpenDiff).toHaveBeenCalledWith("clash");

    const ready = screen.getByRole("region", { name: "Ready to merge" });
    expect(within(ready).getByText("Overlaps Clash")).toBeTruthy();
    fireEvent.click(within(ready).getByRole("button", { name: /Merge queue/ }));
    expect(props.onOpenMergeQueue).toHaveBeenCalled();
  });

  it("launches from its own composer, which can split lines into agents", () => {
    renderBoard();
    const form = screen.getByRole("form", { name: "New agent" });
    expect(within(form).getByRole("button", { name: "One per line" }).getAttribute("aria-pressed")).toBe("false");
  });

  it("says it is loading until the task list arrives", () => {
    renderBoard({ tasks: null });
    expect(screen.getByRole("status").textContent).toBe("Loading agents…");
    expect(screen.queryByRole("region", { name: "Needs you" })).toBeNull();
  });
});

describe("ViewSwitch", () => {
  it("marks the current layout and switches on click", () => {
    const onChange = vi.fn();
    render(<ViewSwitch view="board" onChange={onChange} />);
    expect(screen.getByRole("button", { name: "Board" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "List" }).getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(screen.getByRole("button", { name: "List" }));
    expect(onChange).toHaveBeenCalledWith("list");
  });
});
