import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({ createTask: vi.fn() }));

import { groupTasks } from "./board";
import Home from "./Home";
import { task } from "./testTask";

const BOARD = [
  task({ id: "run", title: "Running one" }),
  task({ id: "limit", title: "Limited", state: "failed", attention: "Usage limit reached" }),
  task({ id: "crash", title: "Crashed", state: "failed", exit_code: 1 }),
  task({ id: "ready", title: "Ready one", state: "finished", commits_ahead: 2, uncommitted_count: 0 }),
  task({ id: "dirty", title: "Dirty one", state: "finished", commits_ahead: 0, uncommitted_count: 3 }),
  task({ id: "done", title: "Landed", state: "finished", merged_into: "main" }),
];

function renderHome(overrides: Partial<Parameters<typeof Home>[0]> = {}) {
  const props = {
    workspace: "/work/acme",
    tasks: BOARD,
    onSelect: vi.fn(),
    onCreated: vi.fn(),
    onOpenFolder: vi.fn(),
    onOpenMergeQueue: vi.fn(),
    focusToken: 0,
    ...overrides,
  };
  render(<Home {...props} />);
  return props;
}

describe("groupTasks", () => {
  it("puts what needs the user first, then live work, then what can land", () => {
    const groups = groupTasks(BOARD);
    expect(groups.map((g) => [g.id, g.tasks.map((t) => t.id)])).toEqual([
      ["attention", ["limit", "crash"]],
      ["running", ["run"]],
      ["ready", ["ready"]],
      ["idle", ["dirty"]],
      ["merged", ["done"]],
    ]);
  });

  it("drops empty groups", () => {
    expect(groupTasks([task({})]).map((g) => g.id)).toEqual(["running"]);
    expect(groupTasks([])).toEqual([]);
  });
});

describe("Home", () => {
  it("summarises the repository and shows each group as cards that open the agent", () => {
    const props = renderHome();
    expect(screen.getByRole("heading", { name: "acme" })).toBeTruthy();
    expect(screen.getByText("1 running")).toBeTruthy();
    expect(screen.getByText("2 need you")).toBeTruthy();
    expect(screen.getByText("1 ready to merge")).toBeTruthy();

    const needs = screen.getByRole("region", { name: "Needs you" });
    const limited = within(needs).getByRole("button", { name: "Limited" });
    expect(within(limited).getByText("Usage limit reached")).toBeTruthy();
    fireEvent.click(limited);
    expect(props.onSelect).toHaveBeenCalledWith("limit");

    const running = within(screen.getByRole("region", { name: "Running" })).getByRole("button", { name: "Running one" });
    expect(within(running).getByText("Editing src/a.rs")).toBeTruthy();
    expect(within(running).getByText("2 files")).toBeTruthy();
    expect(within(running).getByText("Claude Code")).toBeTruthy();
  });

  it("links the ready group to the merge queue", () => {
    const props = renderHome();
    const ready = screen.getByRole("region", { name: "Ready to merge" });
    fireEvent.click(within(ready).getByRole("button", { name: /Merge queue/ }));
    expect(props.onOpenMergeQueue).toHaveBeenCalled();
  });

  it("offers the composer with a folder open, and Open folder without one", () => {
    const { unmount } = render(
      <Home
        workspace="/work/acme"
        tasks={[]}
        onSelect={vi.fn()}
        onCreated={vi.fn()}
        onOpenFolder={vi.fn()}
        onOpenMergeQueue={vi.fn()}
        focusToken={0}
      />,
    );
    expect(screen.getByRole("form", { name: "New agent" })).toBeTruthy();
    expect(screen.getByText(/No agents yet/)).toBeTruthy();
    unmount();

    const props = renderHome({ workspace: null, tasks: [] });
    expect(screen.queryByRole("form", { name: "New agent" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open folder…" }));
    expect(props.onOpenFolder).toHaveBeenCalled();
  });
});
