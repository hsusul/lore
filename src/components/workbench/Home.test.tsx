import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
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
    onNewAgent: vi.fn(),
    onOpenFolder: vi.fn(),
    onOpenMergeQueue: vi.fn(),
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
    // A dashboard, not a composer: the repository and where its agents stand.
    expect(screen.getByRole("heading", { name: "acme" })).toBeTruthy();
    expect(screen.queryByRole("form", { name: "New agent" })).toBeNull();
    expect(screen.getByText("1 running")).toBeTruthy();
    expect(screen.getByText("2 need you")).toBeTruthy();
    expect(screen.getByText("1 ready to merge")).toBeTruthy();
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

  it("starts agents through New agent, with an empty state that says how, and asks for a folder first", () => {
    const { unmount } = render(
      <Home
        workspace="/work/acme"
        tasks={[]}
        onSelect={vi.fn()}
        onNewAgent={vi.fn()}
        onOpenFolder={vi.fn()}
        onOpenMergeQueue={vi.fn()}
      />,
    );
    expect(screen.getAllByText("No agents yet").length).toBeGreaterThan(0);
    // The header button and the empty state's button both open the dialog.
    expect(screen.getAllByRole("button", { name: /New agent/ })).toHaveLength(2);
    unmount();

    const withAgents = renderHome();
    fireEvent.click(screen.getByRole("button", { name: /New agent/ }));
    expect(withAgents.onNewAgent).toHaveBeenCalled();
    cleanup();

    const props = renderHome({ workspace: null, tasks: [] });
    expect(screen.queryByRole("button", { name: /New agent/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open folder…" }));
    expect(props.onOpenFolder).toHaveBeenCalled();
  });
});
