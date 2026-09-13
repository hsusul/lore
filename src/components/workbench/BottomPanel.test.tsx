import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({
  taskActivity: vi.fn(() => Promise.resolve([])),
  listDecisions: vi.fn(),
  onTasksChanged: vi.fn(),
}));

import { listDecisions, onTasksChanged, type DecisionDto } from "../../ipc";
import BottomPanel, { type PanelTab } from "./BottomPanel";
import { task } from "./testTask";

let subscribers: ((ids: string[]) => void)[] = [];

const decision = (overrides: Partial<DecisionDto> = {}): DecisionDto => ({
  at_ms: Date.now() - 60_000,
  task_id: "t1",
  task_title: "Fix parser",
  repo_path: "/repo",
  kind: "auto_handoff",
  detail: "Usage limit reached; handed off to Codex.",
  ...overrides,
});

function renderPanel(tab: PanelTab = "history") {
  const props = {
    height: 220,
    tab,
    onTabChange: vi.fn(),
    onClose: vi.fn(),
    task: task({ state: "finished" }),
    onOpenChange: vi.fn(),
    repoPath: "/repo",
  };
  const view = render(<BottomPanel {...props} />);
  return { props, view };
}

beforeEach(() => {
  subscribers = [];
  vi.mocked(onTasksChanged).mockImplementation((cb) => {
    subscribers.push(cb);
    return Promise.resolve(() => {});
  });
  vi.mocked(listDecisions).mockReset();
});

describe("BottomPanel History", () => {
  it("lists the open repository's decisions newest first and refreshes on tasks_changed", async () => {
    vi.mocked(listDecisions).mockResolvedValue([
      decision(),
      decision({ at_ms: Date.now() - 3600_000, kind: "created", task_title: "Add docs", detail: "Codex started." }),
    ]);
    renderPanel();

    const log = await screen.findByRole("list", { name: "Decision log" });
    const rows = within(log).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("auto handoff");
    expect(rows[0].textContent).toContain("Fix parser");
    expect(rows[0].textContent).toContain("Usage limit reached; handed off to Codex.");
    expect(rows[1].textContent).toContain("created");
    expect(listDecisions).toHaveBeenCalledWith("/repo", expect.any(Number));

    vi.mocked(listDecisions).mockResolvedValue([decision({ kind: "merged", detail: "Merged into main." })]);
    await act(async () => subscribers.forEach((fn) => fn(["t1"])));
    await waitFor(() => expect(listDecisions).toHaveBeenCalledTimes(2));
    expect((await screen.findByRole("list", { name: "Decision log" })).textContent).toContain("Merged into main.");
  });

  it("shows an empty state when nothing has been recorded", async () => {
    vi.mocked(listDecisions).mockResolvedValue([]);
    renderPanel();
    expect(await screen.findByText("No decisions recorded yet.")).toBeTruthy();
  });

  it("shows the History tab beside Output and Changes and cycles with arrow keys", () => {
    vi.mocked(listDecisions).mockResolvedValue([]);
    const { props } = renderPanel("changes");
    const tabs = screen.getByRole("tablist", { name: "Panel views" });
    expect(within(tabs).getAllByRole("tab").map((t) => t.textContent)).toEqual([
      "Agent Output",
      "Changes2",
      "History",
    ]);
    fireEvent.keyDown(tabs, { key: "ArrowRight" });
    expect(props.onTabChange).toHaveBeenCalledWith("history");
  });
});
