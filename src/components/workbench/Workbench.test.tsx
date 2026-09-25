import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";

vi.mock("../../ipc", () => ({
  chooseRepositoryDirectory: vi.fn(),
  openWorkspace: vi.fn(),
  listWorkspaceDir: vi.fn(),
  readWorkspaceFile: vi.fn(),
  listTasks: vi.fn(),
  getTask: vi.fn(),
  listDecisions: vi.fn(() => Promise.resolve([])),
  onTasksChanged: vi.fn(() => Promise.resolve(() => {})),
  createTask: vi.fn(),
  stopTask: vi.fn(),
  discardTask: vi.fn(),
  openTaskWorktree: vi.fn(),
  taskLoadWarning: vi.fn(() => Promise.resolve(null)),
  taskActivity: vi.fn(() => Promise.resolve([])),
  taskDiff: vi.fn(() => Promise.resolve({ text: "", truncated: false })),
  getMergeQueue: vi.fn(() => Promise.resolve(null)),
}));

import {
  chooseRepositoryDirectory,
  discardTask,
  getMergeQueue,
  getTask,
  onTasksChanged,
  listTasks,
  listWorkspaceDir,
  openWorkspace,
  readWorkspaceFile,
  taskActivity,
} from "../../ipc";
import { STORAGE_KEYS } from "./state";
import { task } from "./testTask";
import Workbench from "./Workbench";

const TREE: Record<string, { name: string; rel_path: string; is_dir: boolean }[]> = {
  "": [
    { name: "src", rel_path: "src", is_dir: true },
    { name: "README.md", rel_path: "README.md", is_dir: false },
  ],
  src: [
    { name: "main.ts", rel_path: "src/main.ts", is_dir: false },
    { name: "util.ts", rel_path: "src/util.ts", is_dir: false },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
  window.localStorage.clear();
  window.localStorage.setItem(STORAGE_KEYS.workspace, "/work/repo");
  vi.mocked(openWorkspace).mockImplementation((p) => Promise.resolve(p));
  vi.mocked(listTasks).mockResolvedValue([]);
  vi.mocked(getMergeQueue).mockResolvedValue(null);
  vi.mocked(listWorkspaceDir).mockImplementation((_root, rel) => Promise.resolve(TREE[rel] ?? []));
  vi.mocked(readWorkspaceFile).mockImplementation((_root, rel) =>
    Promise.resolve({ rel_path: rel, text: `// ${rel}\nconst x = 1;\n`, size: 20, truncated: false }),
  );
});

function tabNames() {
  return screen.queryAllByRole("tab").filter((t) => t.closest(".tabs")).map((t) => t.getAttribute("title"));
}

function editorTabs() {
  return within(screen.getByRole("tablist", { name: "Open editors" }));
}

async function openFiles() {
  fireEvent.click(screen.getByRole("tab", { name: "Files" }));
}

async function openFileFromTree(name: string) {
  await openFiles();
  fireEvent.click(await screen.findByRole("treeitem", { name }));
}

describe("Workbench", () => {
  it("restores the workspace and lazily loads folders in the explorer", async () => {
    render(<Workbench />);
    expect(await screen.findByRole("heading", { level: 1, name: "repo" })).toBeTruthy();
    await openFiles();
    await screen.findByRole("treeitem", { name: "src" });
    expect(listWorkspaceDir).toHaveBeenCalledTimes(1);
    expect(listWorkspaceDir).toHaveBeenCalledWith("/work/repo", "");
    expect(screen.queryByRole("treeitem", { name: "main.ts" })).toBeNull();

    const src = screen.getByRole("treeitem", { name: "src" });
    fireEvent.click(src);
    expect(src.getAttribute("aria-expanded")).toBe("true");
    await screen.findByRole("treeitem", { name: "main.ts" });
    expect(listWorkspaceDir).toHaveBeenCalledWith("/work/repo", "src");

    // Collapsing and re-expanding uses the cache.
    fireEvent.click(src);
    expect(screen.queryByRole("treeitem", { name: "main.ts" })).toBeNull();
    fireEvent.click(src);
    await screen.findByRole("treeitem", { name: "main.ts" });
    expect(listWorkspaceDir).toHaveBeenCalledTimes(2);
  });

  it("supports treeview keyboard navigation", async () => {
    render(<Workbench />);
    await openFiles();
    const src = await screen.findByRole("treeitem", { name: "src" });
    act(() => src.focus());
    fireEvent.keyDown(src, { key: "ArrowRight" });
    await screen.findByRole("treeitem", { name: "main.ts" });
    fireEvent.keyDown(src, { key: "ArrowRight" });
    expect(document.activeElement).toBe(screen.getByRole("treeitem", { name: "main.ts" }));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("treeitem", { name: "util.ts" }));
    fireEvent.keyDown(document.activeElement!, { key: "Enter" });
    expect(await screen.findByRole("tab", { name: /util\.ts/ })).toBeTruthy();
    fireEvent.keyDown(screen.getByRole("treeitem", { name: "util.ts" }), { key: "ArrowLeft" });
    expect(document.activeElement).toBe(screen.getByRole("treeitem", { name: "src" }));
    await screen.findByText("// src/util.ts", { selector: ".tok-comment" });
  });

  it("opens files in tabs, activates, and closes with ×, middle-click, and ⌘W", async () => {
    render(<Workbench />);
    expect(screen.getByRole("region", { name: "Overview" })).toBeTruthy();

    await openFileFromTree("README.md");
    const panel = await within(screen.getByRole("main", { name: "Editor" })).findByRole("tabpanel");
    await within(panel).findByText(/const x = 1;/);
    expect(readWorkspaceFile).toHaveBeenCalledWith("/work/repo", "README.md");
    expect(within(panel).getByText(/1\s+2/)).toBeTruthy(); // line-number gutter

    fireEvent.click(screen.getByRole("treeitem", { name: "src" }));
    await openFileFromTree("main.ts");
    // .ts files get lightweight tinting.
    const editor = screen.getByRole("main", { name: "Editor" });
    expect((await within(editor).findByText("const", { selector: ".tok-keyword" })).tagName).toBe("SPAN");
    await openFileFromTree("util.ts");
    await within(editor).findByText("// src/util.ts", { selector: ".tok-comment" });
    expect(tabNames()).toEqual(["/work/repo/README.md", "/work/repo/src/main.ts", "/work/repo/src/util.ts"]);
    expect(editorTabs().getByRole("tab", { selected: true }).getAttribute("title")).toBe("/work/repo/src/util.ts");

    // Re-opening an open file just activates it.
    await openFileFromTree("README.md");
    expect(tabNames()).toHaveLength(3);
    fireEvent.click(screen.getByRole("tab", { name: /main\.ts/ }));
    expect(editorTabs().getByRole("tab", { selected: true }).getAttribute("title")).toBe("/work/repo/src/main.ts");

    fireEvent.keyDown(window, { key: "w", metaKey: true });
    expect(tabNames()).toEqual(["/work/repo/README.md", "/work/repo/src/util.ts"]);
    expect(editorTabs().getByRole("tab", { selected: true }).getAttribute("title")).toBe("/work/repo/src/util.ts");

    fireEvent(
      screen.getByRole("tab", { name: /README\.md/ }),
      new MouseEvent("auxclick", { bubbles: true, button: 1 }),
    );
    expect(tabNames()).toEqual(["/work/repo/src/util.ts"]);

    fireEvent.click(screen.getByRole("button", { name: "Close util.ts" }));
    expect(tabNames()).toEqual([]);
    expect(screen.getByRole("region", { name: "Overview" })).toBeTruthy();
  });

  it("toggles the bottom panel with ⌘J and its close button", async () => {
    render(<Workbench />);
    expect(screen.getByRole("region", { name: "Agents" })).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Panel" })).toBeNull();
    fireEvent.keyDown(window, { key: "j", metaKey: true });
    expect(screen.getByRole("region", { name: "Panel" })).toBeTruthy();
    expect(window.localStorage.getItem(STORAGE_KEYS.panelOpen)).toBe("true");
    fireEvent.keyDown(window, { key: "j", ctrlKey: true });
    expect(screen.queryByRole("region", { name: "Panel" })).toBeNull();
    fireEvent.keyDown(window, { key: "j", metaKey: true });
    fireEvent.click(screen.getByRole("button", { name: "Close panel" }));
    expect(screen.queryByRole("region", { name: "Panel" })).toBeNull();
  });

  it("restores the panel, sidebar view, and sidebar visibility from the last session", async () => {
    window.localStorage.setItem(STORAGE_KEYS.panelOpen, "true");
    window.localStorage.setItem(STORAGE_KEYS.sidebarView, "explorer");
    const { unmount } = render(<Workbench />);
    expect(screen.getByRole("region", { name: "Panel" })).toBeTruthy();
    expect(screen.getByRole("region", { name: "Explorer" })).toBeTruthy();
    await screen.findByRole("treeitem", { name: "src" });
    fireEvent.keyDown(window, { key: "b", metaKey: true });
    expect(window.localStorage.getItem(STORAGE_KEYS.sidebarOpen)).toBe("false");
    unmount();

    render(<Workbench />);
    expect(screen.queryByRole("complementary", { name: "Sidebar" })).toBeNull();
  });

  it("⌘N returns to the overview with the composer focused; ⌘1–9 open agents in sidebar order", async () => {
    vi.mocked(listTasks).mockResolvedValue([
      task({ repo_path: "/work/repo" }),
      task({ id: "t2", title: "Second", repo_path: "/work/repo" }),
    ]);
    render(<Workbench />);
    await screen.findByRole("listitem", { name: "Second" });

    fireEvent.keyDown(window, { key: "2", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Second" })).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Overview" })).toBeNull();

    fireEvent.keyDown(window, { key: "n", metaKey: true });
    expect(screen.getByRole("region", { name: "Overview" })).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByLabelText("Prompt"));

    fireEvent.keyDown(window, { key: "1", ctrlKey: true });
    expect(await screen.findByRole("heading", { name: "Fix parser" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Overview", current: false }));
    expect(screen.getByRole("region", { name: "Overview" })).toBeTruthy();
  });

  it("opens a row's diff as the agent's Changes view, not a separate tab", async () => {
    vi.mocked(listTasks).mockResolvedValue([task({ state: "finished", repo_path: "/work/repo" })]);
    render(<Workbench />);
    const row = await screen.findByRole("listitem", { name: "Fix parser" });
    fireEvent.click(within(row).getByRole("button", { name: "Open diff" }));
    expect(await screen.findByRole("heading", { name: "Fix parser" })).toBeTruthy();
    const views = within(screen.getByRole("tablist", { name: "Agent views" }));
    expect(views.getByRole("tab", { name: /Changes/ }).getAttribute("aria-selected")).toBe("true");
    expect(tabNames()).toEqual([]);

    // Selecting the agent again keeps the view; another agent starts on its activity.
    fireEvent.click(within(row).getByText("Fix parser"));
    expect(views.getByRole("tab", { name: /Changes/ }).getAttribute("aria-selected")).toBe("true");
  });

  it("finds agents in the palette and launches a new agent from typed text", async () => {
    vi.mocked(listTasks).mockResolvedValue([task({ state: "finished", repo_path: "/work/repo" })]);
    render(<Workbench />);
    await screen.findByRole("listitem", { name: "Fix parser" });

    fireEvent.keyDown(window, { key: "k", metaKey: true });
    const input = await screen.findByRole("combobox", { name: "Search agents, commands, and files" });
    fireEvent.change(input, { target: { value: "fix pars" } });
    expect(screen.getAllByRole("option")[0].textContent).toContain("Fix parser");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByRole("heading", { name: "Fix parser" })).toBeTruthy();

    // With an agent selected, its actions are commands.
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "show changes" } });
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });
    const views = within(screen.getByRole("tablist", { name: "Agent views" }));
    expect(views.getByRole("tab", { name: /Changes/ }).getAttribute("aria-selected")).toBe("true");

    fireEvent.keyDown(window, { key: "k", metaKey: true });
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "Add a dark mode toggle" } });
    const options = screen.getAllByRole("option");
    fireEvent.click(options[options.length - 1]);
    expect(screen.getByRole("region", { name: "Overview" })).toBeTruthy();
    const prompt = screen.getByLabelText("Prompt") as HTMLTextAreaElement;
    expect(prompt.value).toBe("Add a dark mode toggle");
    expect(document.activeElement).toBe(prompt);

    // The request is used once: coming back to the overview later neither
    // replays the draft nor steals focus.
    fireEvent.keyDown(window, { key: "1", metaKey: true });
    await screen.findByRole("heading", { name: "Fix parser" });
    fireEvent.click(screen.getAllByRole("button", { name: "Overview" })[0]);
    const again = screen.getByLabelText("Prompt") as HTMLTextAreaElement;
    expect(again.value).toBe("");
    expect(document.activeElement).not.toBe(again);
  });

  it("toasts when another agent finishes while Lore is in front, but not for the agent on screen", async () => {
    const subscribers: ((ids: string[]) => void)[] = [];
    vi.mocked(onTasksChanged).mockImplementation((cb) => {
      subscribers.push(cb);
      return Promise.resolve(() => {});
    });
    const focus = vi.spyOn(document, "hasFocus").mockReturnValue(true);
    onTestFinished(() => focus.mockRestore());
    vi.mocked(listTasks).mockResolvedValue([
      task({ repo_path: "/work/repo" }),
      task({ id: "t2", title: "Second", repo_path: "/work/repo" }),
    ]);
    render(<Workbench />);
    fireEvent.click(within(await screen.findByRole("listitem", { name: "Second" })).getByText("Second"));

    vi.mocked(getTask).mockImplementation((id) =>
      Promise.resolve(
        id === "t1"
          ? task({ state: "finished", repo_path: "/work/repo", last_activity: "Parser fixed" })
          : task({ id: "t2", title: "Second", state: "finished", repo_path: "/work/repo" }),
      ),
    );
    await act(async () => subscribers.forEach((cb) => cb(["t1", "t2"])));
    const toastEl = (await screen.findByText("Fix parser finished")).closest("[data-sonner-toast]") as HTMLElement;
    expect(within(toastEl).getByText("Parser fixed")).toBeTruthy();
    expect(screen.queryByText("Second finished")).toBeNull();
    fireEvent.click(within(toastEl).getByRole("button", { name: "Open" }));
    expect(await screen.findByRole("heading", { name: "Fix parser" })).toBeTruthy();
  });

  it("switches the sidebar between agents and files", async () => {
    render(<Workbench />);
    expect(screen.getByRole("region", { name: "Agents" })).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Explorer" })).toBeNull();
    await openFiles();
    await screen.findByRole("treeitem", { name: "src" });
    expect(screen.queryByRole("region", { name: "Agents" })).toBeNull();
    fireEvent.click(screen.getByRole("tab", { name: "Agents" }));
    expect(screen.getByRole("region", { name: "Agents" })).toBeTruthy();
    expect(screen.getByRole("main", { name: "Editor" })).toBeTruthy();
  });

  it("selecting an agent opens its tab, output, and changes; discard closes its tabs", async () => {
    vi.mocked(listTasks).mockResolvedValue([task({ state: "finished", repo_path: "/work/repo" })]);
    vi.mocked(taskActivity).mockResolvedValue([{ kind: "message", text: "All done" }]);
    vi.mocked(discardTask).mockResolvedValue(undefined);
    render(<Workbench />);

    const row = await screen.findByRole("listitem", { name: "Fix parser" });
    fireEvent.click(within(row).getByText("Fix parser"));

    expect(await screen.findByRole("heading", { name: "Fix parser" })).toBeTruthy();
    expect(screen.queryByRole("tab", { name: /Agent: Fix parser/ })).toBeNull();
    await screen.findByText("All done");
    expect(screen.getByText("lore/fix-parser", { selector: ".statusbar .mono" })).toBeTruthy();

    fireEvent.keyDown(window, { key: "j", metaKey: true });
    const panel = screen.getByRole("region", { name: "Panel" });
    fireEvent.click(within(panel).getByRole("tab", { name: /Changes/ }));
    fireEvent.click(within(panel).getByRole("button", { name: /a\.rs/ }));
    expect(readWorkspaceFile).toHaveBeenCalledWith("/lore/worktrees/fix-parser", "src/a.rs");
    const fileTab = await screen.findByRole("tab", { name: /a\.rs/ });
    expect(within(fileTab).getByText("Fix parser")).toBeTruthy(); // worktree scope suffix

    vi.mocked(listTasks).mockResolvedValue([]);
    fireEvent.click(within(row).getByRole("button", { name: "Discard" }));
    fireEvent.click(within(row).getByRole("button", { name: "Confirm discard" }));
    await waitFor(() => expect(discardTask).toHaveBeenCalledWith("t1"));
    await waitFor(() => expect(tabNames()).toEqual([]));
  });

  it("scopes tasks, counts, and the explorer picker to the open repository", async () => {
    vi.mocked(listTasks).mockResolvedValue([
      task({ state: "running", repo_path: "/work/repo" }),
      task({ id: "t2", title: "Other repo", repo_path: "/work/other", state: "running" }),
    ]);
    render(<Workbench />);

    await screen.findByRole("listitem", { name: "Fix parser" });
    expect(screen.queryByRole("listitem", { name: "Other repo" })).toBeNull();
    expect(screen.getByRole("button", { name: /1 agent running/ })).toBeTruthy();
    await openFiles();
    expect(
      within(screen.getByLabelText("Explorer scope")).queryByText("Worktree: Other repo"),
    ).toBeNull();

    fireEvent.click(screen.getByRole("tab", { name: "Agents" }));
    fireEvent.click(screen.getByRole("button", { name: /All repositories/ }));
    expect(await screen.findByRole("listitem", { name: "Other repo" })).toBeTruthy();
    expect(screen.getByRole("button", { name: /2 agents running/ })).toBeTruthy();
    await openFiles();
    expect(within(screen.getByLabelText("Explorer scope")).getByText("Worktree: Other repo")).toBeTruthy();
    expect(window.localStorage.getItem(STORAGE_KEYS.allRepos)).toBe("true");
  });

  it("picks up a merge queue already running in the opened repository and pauses its agents' git actions", async () => {
    vi.mocked(listTasks).mockResolvedValue([task({ state: "finished", repo_path: "/work/repo" })]);
    vi.mocked(getMergeQueue).mockResolvedValue({
      repo_path: "/work/repo",
      running: true,
      test_command: null,
      items: [{ task_id: "t1", title: "Fix parser", status: "merging", detail: null }],
    });
    render(<Workbench />);
    fireEvent.click(within(await screen.findByRole("listitem", { name: "Fix parser" })).getByText("Fix parser"));

    expect(
      await screen.findByText("Commit, merge, and continue are paused while this repository’s merge queue runs."),
    ).toBeTruthy();
    expect(getMergeQueue).toHaveBeenCalledWith("/work/repo");
    expect(screen.getByRole("button", { name: /Merge into/ }).matches(":disabled")).toBe(true);
    fireEvent.keyDown(window, { key: "j", metaKey: true });
    const panelTabs = within(screen.getByRole("tablist", { name: "Panel views" }));
    expect(panelTabs.getByRole("tab", { name: /Merge queue/ }).textContent).toBe("Merge queue (running)");
  });

  it("opens a folder through the native picker and resolves the repo top-level", async () => {
    window.localStorage.clear();
    vi.mocked(chooseRepositoryDirectory).mockResolvedValue("/work/repo/src");
    vi.mocked(openWorkspace).mockResolvedValue("/work/repo");
    render(<Workbench />);
    fireEvent.click(screen.getAllByRole("button", { name: "Open folder…" })[0]);
    expect(await screen.findByRole("heading", { level: 1, name: "repo" })).toBeTruthy();
    expect(openWorkspace).toHaveBeenCalledWith("/work/repo/src");
    expect(window.localStorage.getItem(STORAGE_KEYS.workspace)).toBe("/work/repo");
    await openFiles();
    await screen.findByRole("treeitem", { name: "src" });
  });

  it("shows a dismissible alert when the chosen folder is not a git repository", async () => {
    window.localStorage.clear();
    vi.mocked(chooseRepositoryDirectory).mockResolvedValue("/tmp/not-a-repo");
    vi.mocked(openWorkspace).mockRejectedValue("That folder isn't inside a git repository");
    render(<Workbench />);
    fireEvent.click(screen.getAllByRole("button", { name: "Open folder…" })[0]);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("That folder isn't inside a git repository");
    expect(alert.className).toContain("wb-banner");
    fireEvent.click(within(alert).getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("command palette opens with ⌘K, lists loaded files, and runs commands", async () => {
    render(<Workbench />);
    await waitFor(() => expect(listWorkspaceDir).toHaveBeenCalled());
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    const input = await screen.findByRole("combobox", { name: "Search agents, commands, and files" });
    fireEvent.change(input, { target: { value: "readme" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(await screen.findByRole("tab", { name: /README\.md/ })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    expect(await screen.findByRole("combobox", { name: "Search agents, commands, and files" })).toBeTruthy();
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Search agents, commands, and files" }), { key: "Escape" });

    fireEvent.keyDown(window, { key: "p", metaKey: true });
    fireEvent.change(await screen.findByRole("combobox", { name: "Search agents, commands, and files" }), { target: { value: "toggle panel" } });
    await act(async () => {
      fireEvent.keyDown(screen.getByRole("combobox", { name: "Search agents, commands, and files" }), { key: "Enter" });
    });
    expect(screen.getByRole("region", { name: "Panel" })).toBeTruthy();
  });
});
