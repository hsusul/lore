import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({ createTask: vi.fn() }));

import { createTask } from "../../ipc";
import AgentsPanel from "./AgentsPanel";
import { task } from "./testTask";

function renderPanel(overrides: Partial<Parameters<typeof AgentsPanel>[0]> = {}) {
  const props = {
    workspace: "/repo",
    tasks: [task({})],
    listError: null,
    loadWarning: null,
    selectedTaskId: null,
    onSelect: vi.fn(),
    onStop: vi.fn(() => Promise.resolve()),
    onDiscard: vi.fn(() => Promise.resolve()),
    onReveal: vi.fn(() => Promise.resolve()),
    onOpenDiff: vi.fn(),
    onCreated: vi.fn(),
    onOpenFolder: vi.fn(),
    focusToken: 0,
    allRepos: false,
    onAllReposChange: vi.fn(),
    hiddenCount: 0,
    ...overrides,
  };
  render(<AgentsPanel {...props} />);
  return props;
}

function chooseAgent(name: "Claude Code" | "Codex") {
  fireEvent.click(screen.getByLabelText("Agent"));
  fireEvent.click(screen.getByRole("menuitem", { name: /^Agent\b/ }));
  fireEvent.click(screen.getByRole("menuitemradio", { name }));
}

function openPermission() {
  fireEvent.click(screen.getByLabelText("Agent"));
  fireEvent.click(screen.getByRole("menuitem", { name: /^Permission\b/ }));
}

beforeEach(() => {
  vi.mocked(createTask).mockReset();
});

describe("AgentsPanel", () => {
  it("renders tasks with state text, agent, branch, and counts", () => {
    renderPanel({
      tasks: [
        task({}),
        task({ id: "t2", title: "Add docs", agent: "codex", state: "failed", exit_code: 1, changed_files: [] }),
      ],
    });
    const running = screen.getByRole("listitem", { name: "Fix parser" });
    expect(within(running).getByText("Running")).toBeTruthy();
    expect(within(running).getByText("Claude Code")).toBeTruthy();
    expect(within(running).getByText("lore/fix-parser")).toBeTruthy();
    expect(within(running).getByText("2 commits ahead")).toBeTruthy();
    expect(within(running).getByText("2 files changed")).toBeTruthy();
    expect(within(running).getByRole("button", { name: "Stop" })).toBeTruthy();

    const failed = screen.getByRole("listitem", { name: "Add docs" });
    expect(within(failed).getByText("Failed (exit 1)")).toBeTruthy();
    expect(within(failed).getByText("Codex")).toBeTruthy();
    expect(within(failed).queryByRole("button", { name: "Stop" })).toBeNull();
  });

  it("launches with the workspace root as the repository (⌘Enter submits)", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ id: "new" }));
    const props = renderPanel({ tasks: [] });

    fireEvent.change(screen.getByLabelText("Title"), { target: { value: " Fix parser " } });
    chooseAgent("Codex");
    const prompt = screen.getByLabelText("Prompt");
    fireEvent.change(prompt, { target: { value: "fix it" } });
    fireEvent.keyDown(prompt, { key: "Enter", metaKey: true });

    await waitFor(() =>
      expect(createTask).toHaveBeenCalledWith({
        repo_path: "/repo",
        title: "Fix parser",
        prompt: "fix it",
        agent: "codex",
        permission: "edits",
        auto_handoff: true,
      }),
    );
    await waitFor(() => expect(props.onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "new" })));
  });

  it("sends the chosen permission, defaulting to Edits", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ id: "new" }));
    renderPanel({ tasks: [] });
    openPermission();
    const group = screen.getByRole("radiogroup", { name: "Permission" });
    const edits = within(group).getByRole("radio", { name: "Edits" });
    const auto = within(group).getByRole("radio", { name: "Auto" });
    expect(edits.getAttribute("aria-checked")).toBe("true");
    expect(edits.getAttribute("title")).toBe("Claude may edit files but not run shell commands.");
    expect(auto.getAttribute("title")).toMatch(/safety classifier/);

    fireEvent.click(auto);
    // The picker returns to the root pane; Auto is the shown permission.
    expect(screen.queryByRole("radiogroup", { name: "Permission" })).toBeNull();
    expect(screen.getByRole("menuitem", { name: /^Permission\b/ }).textContent).toContain("Auto");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "T" } });
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "P" } });
    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    await waitFor(() =>
      expect(createTask).toHaveBeenCalledWith(expect.objectContaining({ permission: "auto" })),
    );
  });

  it("shows compact attention, overlap, uncommitted, and merged indicators on rows", () => {
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
    expect(within(first).getByLabelText("Overlaps with 1 task").textContent).toBe("1");
    expect(within(first).getByLabelText("3 uncommitted")).toBeTruthy();
    expect(within(first).queryByLabelText("Merged into main")).toBeNull();

    const merged = screen.getByRole("listitem", { name: "Merged one" });
    expect(within(merged).getByLabelText("Merged into main")).toBeTruthy();
    expect(within(merged).queryByLabelText("Needs attention")).toBeNull();
    expect(within(merged).queryByLabelText(/uncommitted/)).toBeNull();
  });

  it("disables the form with a hint when no workspace is open", () => {
    const props = renderPanel({ workspace: null, tasks: [] });
    expect(screen.getByRole("button", { name: "Launch" }).matches(":disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Open Folder…" }));
    expect(props.onOpenFolder).toHaveBeenCalled();
  });

  it("shows backend errors inline", async () => {
    vi.mocked(createTask).mockRejectedValue("not a git repository: /repo");
    renderPanel({ tasks: [] });
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "T" } });
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "P" } });
    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    expect((await screen.findByRole("alert")).textContent).toBe("not a git repository: /repo");
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

  it("parses claims into chips and sends them with auto-handoff", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ id: "new" }));
    renderPanel({ tasks: [] });
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "T" } });
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "P" } });
    fireEvent.change(screen.getByLabelText("Owns (files or folders)"), {
      target: { value: " src/parser/ , docs/SCHEMA.md\n./src/parser/ \n" },
    });

    // Comma- and newline-separated, trimmed, deduplicated, shown as chips.
    const chips = within(screen.getByRole("list", { name: "Owned paths" })).getAllByRole("listitem");
    expect(chips.map((li) => li.textContent)).toEqual(["src/parser/", "docs/SCHEMA.md"]);
    expect(screen.getByText(/Other agents are told not to touch these/)).toBeTruthy();

    fireEvent.click(screen.getByLabelText("Agent"));
    const autoHandoff = screen.getByLabelText("Auto-handoff on usage limit");
    expect(autoHandoff.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(autoHandoff);

    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    await waitFor(() =>
      expect(createTask).toHaveBeenCalledWith(
        expect.objectContaining({ claims: ["src/parser/", "docs/SCHEMA.md"], auto_handoff: false }),
      ),
    );
  });

  it("omits claims entirely when the field is empty", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ id: "new" }));
    renderPanel({ tasks: [] });
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "T" } });
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "P" } });
    fireEvent.change(screen.getByLabelText("Owns (files or folders)"), { target: { value: " , \n " } });
    expect(screen.queryByRole("list", { name: "Owned paths" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    await waitFor(() => expect(createTask).toHaveBeenCalled());
    expect(vi.mocked(createTask).mock.calls[0][0]).not.toHaveProperty("claims");
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

  it("marks a task whose auto-handoff is off", () => {
    renderPanel({
      tasks: [task({ auto_handoff: false }), task({ id: "t2", title: "Auto one", auto_handoff: true })],
    });
    const manual = screen.getByRole("listitem", { name: "Fix parser" });
    expect(within(manual).getByText("manual handoff")).toBeTruthy();
    expect(within(screen.getByRole("listitem", { name: "Auto one" })).queryByText("manual handoff")).toBeNull();
  });

  it("shows the task load warning", () => {
    renderPanel({ loadWarning: "tasks.json is corrupt" });
    expect(screen.getByRole("alert").textContent).toBe("tasks.json is corrupt");
  });
});
