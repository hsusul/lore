import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import AgentsPanel from "./AgentsPanel";
import { task } from "./testTask";

function renderPanel(overrides: Partial<Parameters<typeof AgentsPanel>[0]> = {}) {
  const props = {
    workspace: "/repo",
    tasks: [task({})],
    listError: null,
    loadWarning: null,
    selectedTaskId: null,
    overviewActive: true,
    onShowOverview: vi.fn(),
    onNewAgent: vi.fn(),
    onSelect: vi.fn(),
    onStop: vi.fn(() => Promise.resolve()),
    onDiscard: vi.fn(() => Promise.resolve()),
    onReveal: vi.fn(() => Promise.resolve()),
    onOpenDiff: vi.fn(),
    allRepos: false,
    onAllReposChange: vi.fn(),
    hiddenCount: 0,
    ...overrides,
  };
  render(<AgentsPanel {...props} />);
  return props;
}

describe("AgentsPanel", () => {
  it("renders tasks with state text and agent", () => {
    renderPanel({
      tasks: [
        task({}),
        task({ id: "t2", title: "Add docs", agent: "codex", state: "failed", exit_code: 1, changed_files: [] }),
      ],
    });
    const running = screen.getByRole("listitem", { name: "Fix parser" });
    expect(within(running).getByText("Running")).toBeTruthy();
    expect(within(running).getByText("Claude Code")).toBeTruthy();
    expect(within(running).queryByText("lore/fix-parser")).toBeNull();
    expect(within(running).getByRole("button", { name: "Fix parser" }).getAttribute("title")).toContain(
      "lore/fix-parser",
    );
    expect(within(running).getByRole("button", { name: "Stop" })).toBeTruthy();

    const failed = screen.getByRole("listitem", { name: "Add docs" });
    expect(within(failed).getByText("Failed")).toBeTruthy();
    expect(within(failed).queryByText("Failed (exit 1)")).toBeNull();
    expect(within(failed).getByText("Codex")).toBeTruthy();
    expect(within(failed).queryByRole("button", { name: "Stop" })).toBeNull();
  });


  it("keeps branch and counts in the tooltip, and shows attention on the row", () => {
    renderPanel({
      tasks: [
        task({
          attention: "Usage limit reached",
          uncommitted_count: 3,
          overlaps: [{ task_id: "t2", title: "Other", files: ["src/a.rs"] }],
        }),
        task({ id: "t2", title: "Merged one", state: "finished", merged_into: "main" }),
      ],
    });
    const first = screen.getByRole("listitem", { name: "Fix parser" });
    expect(within(first).getByLabelText("Needs attention").getAttribute("title")).toBe("Usage limit reached");
    expect(within(first).queryByLabelText(/Overlaps/)).toBeNull();
    expect(within(first).queryByLabelText(/uncommitted/)).toBeNull();
    expect(within(first).getByRole("button", { name: "Fix parser" }).getAttribute("title")).toMatch(
      /lore\/fix-parser.*3 uncommitted.*Overlaps 1 task/,
    );

    const merged = screen.getByRole("listitem", { name: "Merged one" });
    expect(within(merged).getByText("Merged")).toBeTruthy();
    expect(within(merged).queryByLabelText("Needs attention")).toBeNull();
    expect(within(merged).queryByLabelText(/uncommitted/)).toBeNull();
    expect(within(merged).getByRole("button", { name: "Merged one" }).getAttribute("title")).toContain(
      "Merged into main",
    );
  });


  it("selects, stops, opens the diff, and reveals from a row", async () => {
    const props = renderPanel();
    const row = screen.getByRole("listitem", { name: "Fix parser" });
    fireEvent.click(within(row).getByText("Fix parser"));
    expect(props.onSelect).toHaveBeenCalledWith("t1");
    fireEvent.click(within(row).getByRole("button", { name: "Open diff" }));
    expect(props.onOpenDiff).toHaveBeenCalledWith("t1");
    fireEvent.click(within(row).getByRole("button", { name: "Reveal in Finder" }));
    await waitFor(() => expect(props.onReveal).toHaveBeenCalledWith("t1"));
    fireEvent.click(within(row).getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(props.onStop).toHaveBeenCalledWith("t1"));
  });


  it("requires an inline confirmation before discarding", async () => {
    const props = renderPanel();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(props.onDiscard).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(props.onDiscard).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm discard" }));
    await waitFor(() => expect(props.onDiscard).toHaveBeenCalledWith("t1"));
  });


  it("toggles the repository scope and counts what it hides", () => {
    const props = renderPanel({ tasks: [], hiddenCount: 3 });
    const toggle = screen.getByRole("button", { name: /All repositories/ });
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    expect(toggle.textContent).toContain("3");
    expect(screen.getByText("No agents in this repository (3 in others).")).toBeTruthy();
    fireEvent.click(toggle);
    expect(props.onAllReposChange).toHaveBeenCalledWith(true);
  });


  it("keeps manual handoff off the row and in the tooltip", () => {
    renderPanel({
      tasks: [task({ auto_handoff: false }), task({ id: "t2", title: "Auto one", auto_handoff: true })],
    });
    const manual = screen.getByRole("listitem", { name: "Fix parser" });
    expect(within(manual).queryByText("manual handoff")).toBeNull();
    expect(within(manual).getByRole("button", { name: "Fix parser" }).getAttribute("title")).toContain(
      "manual handoff",
    );
    expect(within(screen.getByRole("listitem", { name: "Auto one" })).queryByText("manual handoff")).toBeNull();
  });


  it("shows the task load warning", () => {
    renderPanel({ loadWarning: "tasks.json is corrupt" });
    expect(screen.getByRole("alert").textContent).toBe("tasks.json is corrupt");
  });

  it("previews the latest activity, or what needs the user, under each row", () => {
    renderPanel({
      tasks: [
        task({ last_activity: "Editing src/a.rs" }),
        task({ id: "t2", title: "Limited", state: "failed", attention: "Usage limit reached", last_activity: "npm test" }),
        task({ id: "t3", title: "Landed", state: "finished", merged_into: "main", last_activity: "Done" }),
      ],
    });
    expect(within(screen.getByRole("listitem", { name: "Fix parser" })).getByText("Editing src/a.rs")).toBeTruthy();
    const limited = screen.getByRole("listitem", { name: "Limited" });
    expect(within(limited).getByText("Usage limit reached", { selector: ".agent-row__preview" })).toBeTruthy();
    expect(within(limited).queryByText("npm test")).toBeNull();
    expect(within(screen.getByRole("listitem", { name: "Landed" })).queryByText("Done")).toBeNull();
    expect(within(screen.getByRole("listitem", { name: "Fix parser" })).getByText("2 files")).toBeTruthy();
  });

  it("offers the overview and a new agent, with counts of what is running and needs you", () => {
    const props = renderPanel({
      overviewActive: true,
      tasks: [task({}), task({ id: "t2", title: "Stuck", state: "failed", exit_code: 1 })],
    });
    const overview = screen.getByRole("button", { name: /Overview/ });
    expect(overview.getAttribute("aria-current")).toBe("true");
    expect(within(overview).getByTitle("Running").textContent).toBe("1");
    expect(within(overview).getByTitle("Need you").textContent).toBe("1");
    fireEvent.click(overview);
    expect(props.onShowOverview).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    expect(props.onNewAgent).toHaveBeenCalled();
  });

  it("is one tab stop, moved with the arrow keys", () => {
    renderPanel({
      selectedTaskId: "t2",
      tasks: [task({}), task({ id: "t2", title: "Second" }), task({ id: "t3", title: "Third" })],
    });
    const main = (name: string) => screen.getByRole("button", { name });
    expect(main("Fix parser").tabIndex).toBe(-1);
    expect(main("Second").tabIndex).toBe(0);
    // Row shortcuts stay out of the tab order; the agent view and ⌘K have them.
    expect(within(screen.getByRole("listitem", { name: "Second" })).getByRole("button", { name: "Open diff" }).tabIndex).toBe(-1);

    main("Second").focus();
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(main("Third"));
    expect(main("Third").tabIndex).toBe(0);
    fireEvent.keyDown(document.activeElement!, { key: "Home" });
    expect(document.activeElement).toBe(main("Fix parser"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
    expect(document.activeElement).toBe(main("Fix parser"));
  });
});
