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
  it("lists every agent, what needs you first, and opens one on click", () => {
    const props = renderHome();
    expect(screen.getByRole("heading", { name: "What should an agent work on?" })).toBeTruthy();
    const list = screen.getByRole("tabpanel", { name: "All" });
    const rows = within(list).getAllByRole("button");
    expect(rows.map((r) => r.getAttribute("aria-label"))).toEqual([
      "Limited",
      "Crashed",
      "Running one",
      "Ready one",
      "Dirty one",
      "Landed",
    ]);
    const limited = within(list).getByRole("button", { name: "Limited" });
    expect(within(limited).getByRole("img", { name: "Needs you" })).toBeTruthy();
    expect(within(limited).getByText("Usage limit reached")).toBeTruthy();
    fireEvent.click(limited);
    expect(props.onSelect).toHaveBeenCalledWith("limit");

    const running = within(list).getByRole("button", { name: "Running one" });
    expect(within(running).getByText("Editing src/a.rs")).toBeTruthy();
    expect(within(running).getByText("2 files")).toBeTruthy();
    expect(within(running).getByText("Claude Code")).toBeTruthy();
  });

  it("filters by what the agents need, with counts, and links ready work to the merge queue", () => {
    const props = renderHome();
    const filters = screen.getByRole("tablist", { name: "Filter agents" });
    expect(within(filters).getAllByRole("tab").map((t) => t.textContent)).toEqual([
      "All6",
      "Needs you2",
      "Running1",
      "Ready to merge1",
      "To review1",
      "Merged1",
    ]);
    fireEvent.click(within(filters).getByRole("tab", { name: /Needs you/ }));
    const list = screen.getByRole("tabpanel", { name: "Needs you" });
    expect(within(list).getAllByRole("button").map((r) => r.getAttribute("aria-label"))).toEqual(["Limited", "Crashed"]);
    fireEvent.click(screen.getByRole("button", { name: /Merge queue/ }));
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
