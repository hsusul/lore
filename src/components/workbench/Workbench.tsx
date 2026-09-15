import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";

import {
  chooseRepositoryDirectory,
  commitTask,
  continueTask,
  discardTask,
  mergeTask,
  openTaskWorktree,
  openWorkspace,
  stopTask,
  type ContinueTaskRequest,
  type TaskDto,
} from "../../ipc";
import AgentsPanel from "./AgentsPanel";
import AgentView from "./AgentView";
import BottomPanel, { type PanelTab } from "./BottomPanel";
import CommandPalette, { type PaletteItem } from "./CommandPalette";
import DiffView from "./DiffView";
import EditorTabs from "./EditorTabs";
import FileView from "./FileView";
import { Mark, PanelIcon, SearchIcon, SidebarIcon } from "./icons";
import type { SidebarView } from "./ActivityBar";
import Sash from "./Sash";
import { ExplorerPanel } from "./Sidebar";
import {
  PANEL_MAX,
  PANEL_MIN,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  STORAGE_KEYS,
  baseName,
  diffTab,
  errorText,
  fileTab,
  initialTabs,
  readStored,
  readStoredNumber,
  tabsReducer,
  writeStored,
  type Tab,
} from "./state";
import StatusBar from "./StatusBar";
import { dirKey, useDirCache } from "./useDirCache";
import { useMergeQueues } from "./useMergeQueue";
import { useTasks } from "./useTasks";

/** IDE workbench: sidebar (Agents / Files), editor, optional bottom panel. No activity bar. */
export default function Workbench() {
  const { tasks, listError, loadWarning, refresh } = useTasks();
  const dirs = useDirCache();

  const [workspace, setWorkspace] = useState<string | null>(null);
  const mergeQueues = useMergeQueues(workspace, refresh);
  const [workspaceError, setWorkspaceError] = useState<string | null>(null);
  const [{ tabs, activeKey }, dispatch] = useReducer(tabsReducer, initialTabs);
  const [sidebarWidth, setSidebarWidth] = useState(() =>
    readStoredNumber(STORAGE_KEYS.sidebarWidth, 260, SIDEBAR_MIN, SIDEBAR_MAX),
  );
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [sidebarView, setSidebarView] = useState<SidebarView>("agents");
  const [panelOpen, setPanelOpen] = useState(false);
  const [panelHeight, setPanelHeight] = useState(() =>
    readStoredNumber(STORAGE_KEYS.panelHeight, 280, PANEL_MIN, PANEL_MAX),
  );
  const [panelTab, setPanelTab] = useState<PanelTab>("output");
  const [allRepos, setAllRepos] = useState(() => readStored(STORAGE_KEYS.allRepos) === "true");
  const [scope, setScope] = useState("repo");
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [newAgentToken, setNewAgentToken] = useState(0);

  useEffect(() => {
    const stored = readStored(STORAGE_KEYS.workspace);
    if (!stored) return;
    let cancelled = false;
    openWorkspace(stored)
      .then((top) => {
        if (!cancelled) setWorkspace(top);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (workspace) void dirs.load(workspace, "");
  }, [workspace, dirs.load]);

  useEffect(() => writeStored(STORAGE_KEYS.sidebarWidth, String(sidebarWidth)), [sidebarWidth]);
  useEffect(() => writeStored(STORAGE_KEYS.panelHeight, String(panelHeight)), [panelHeight]);
  useEffect(() => writeStored(STORAGE_KEYS.panelOpen, String(panelOpen)), [panelOpen]);
  useEffect(() => writeStored(STORAGE_KEYS.allRepos, String(allRepos)), [allRepos]);

  const taskById = useMemo(() => new Map((tasks ?? []).map((t) => [t.id, t])), [tasks]);
  const visibleTasks = useMemo(() => {
    if (tasks === null) return null;
    if (allRepos || workspace === null) return tasks;
    return tasks.filter((t) => t.repo_path === workspace);
  }, [tasks, allRepos, workspace]);
  const hiddenCount = (tasks?.length ?? 0) - (visibleTasks?.length ?? 0);
  const runningCount = (visibleTasks ?? []).filter((t) => t.state === "running").length;
  const attentionCount = (visibleTasks ?? []).filter((t) => t.attention).length;
  const selectedTask = selectedTaskId ? taskById.get(selectedTaskId) : undefined;
  const scopeTask = scope === "repo" ? undefined : taskById.get(scope);
  const scopeRoot = scope === "repo" ? workspace : (scopeTask?.worktree_path ?? null);

  useEffect(() => {
    if (scope !== "repo" && visibleTasks && !visibleTasks.some((t) => t.id === scope)) setScope("repo");
  }, [scope, visibleTasks]);

  const openFolder = useCallback(async () => {
    setWorkspaceError(null);
    try {
      const picked = await chooseRepositoryDirectory();
      if (picked === null) return;
      const top = await openWorkspace(picked);
      setWorkspace(top);
      writeStored(STORAGE_KEYS.workspace, top);
      setScope("repo");
    } catch (e) {
      setWorkspaceError(errorText(e));
    }
  }, []);

  const tasksRef = useRef(tasks);
  tasksRef.current = tasks;

  const openFile = useCallback((root: string, relPath: string) => {
    const owner = (tasksRef.current ?? []).find((t) => t.worktree_path === root);
    dispatch({ type: "open", tab: fileTab(root, relPath, owner ? owner.title : null) });
  }, []);

  const selectTask = useCallback((id: string) => {
    setSelectedTaskId(id);
    dispatch({ type: "clearActive" });
  }, []);

  const openDiff = useCallback((id: string) => {
    setSelectedTaskId(id);
    dispatch({ type: "open", tab: diffTab(id) });
  }, []);

  const handleStop = useCallback(
    async (id: string) => {
      await stopTask(id);
      await refresh();
    },
    [refresh],
  );

  const handleDiscard = useCallback(
    async (id: string) => {
      const worktree = tasksRef.current?.find((t) => t.id === id)?.worktree_path;
      await discardTask(id);
      dispatch({
        type: "closeWhere",
        predicate: (tab) => (tab.kind === "file" ? tab.root === worktree : tab.taskId === id),
      });
      setSelectedTaskId((current) => (current === id ? null : current));
      await refresh();
    },
    [refresh],
  );

  const handleContinue = useCallback(
    async (request: ContinueTaskRequest) => {
      await continueTask(request);
      await refresh();
    },
    [refresh],
  );

  const handleCommit = useCallback(
    async (id: string, message: string, force?: boolean) => {
      try {
        await commitTask(id, message, force);
      } finally {
        await refresh();
      }
    },
    [refresh],
  );

  const handleMerge = useCallback(
    async (id: string) => {
      try {
        return await mergeTask(id);
      } finally {
        await refresh();
      }
    },
    [refresh],
  );

  const handleReveal = useCallback((id: string) => openTaskWorktree(id), []);

  const handleCreated = useCallback(
    (task: TaskDto) => {
      void refresh();
      selectTask(task.id);
    },
    [refresh, selectTask],
  );

  const toggleSidebar = useCallback(() => setSidebarOpen((open) => !open), []);
  const togglePanel = useCallback(() => setPanelOpen((open) => !open), []);
  const showAgents = useCallback(() => {
    setSidebarOpen(true);
    setSidebarView("agents");
  }, []);
  const showFiles = useCallback(() => {
    setSidebarOpen(true);
    setSidebarView("explorer");
  }, []);

  const newAgent = useCallback(() => {
    showAgents();
    setNewAgentToken((n) => n + 1);
  }, [showAgents]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      switch (e.key.toLowerCase()) {
        case "k":
        case "p":
          e.preventDefault();
          setPaletteOpen(true);
          break;
        case "w":
          e.preventDefault();
          dispatch({ type: "closeActive" });
          break;
        case "j":
          e.preventDefault();
          togglePanel();
          break;
        case "b":
          e.preventDefault();
          toggleSidebar();
          break;
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleSidebar, togglePanel]);

  const taskTitle = useCallback((id: string) => taskById.get(id)?.title ?? "Discarded agent", [taskById]);

  const renderTab = (tab: Tab, active: boolean) => {
    switch (tab.kind) {
      case "file":
        return <FileView root={tab.root} relPath={tab.relPath} />;
      case "diff":
        return <DiffView taskId={tab.taskId} title={taskTitle(tab.taskId)} />;
      case "agent": {
        const task = taskById.get(tab.taskId);
        return (
          <AgentView
            active={active}
            taskId={tab.taskId}
            task={task}
            mergeQueueRunning={task ? mergeQueues.isRunning(task.repo_path) : false}
            onStop={handleStop}
            onDiscard={handleDiscard}
            onReveal={handleReveal}
            onOpenDiff={openDiff}
            onContinue={handleContinue}
            onCommit={handleCommit}
            onMerge={handleMerge}
            onSelectTask={selectTask}
          />
        );
      }
    }
  };

  const activeTab = tabs.find((t) => t.key === activeKey);
  const activeFile = activeTab?.kind === "file" ? { root: activeTab.root, relPath: activeTab.relPath } : null;

  const paletteItems = useMemo<PaletteItem[]>(() => {
    const commands: PaletteItem[] = [
      { id: "cmd:open-folder", group: "Command", label: "Open Folder…", run: () => void openFolder() },
      { id: "cmd:new-agent", group: "Command", label: "New Agent", run: newAgent },
      { id: "cmd:toggle-sidebar", group: "Command", label: "Toggle Sidebar", hint: "⌘B", run: toggleSidebar },
      { id: "cmd:show-files", group: "Command", label: "Show Files", run: showFiles },
      { id: "cmd:show-agents", group: "Command", label: "Show Agents", run: showAgents },
      { id: "cmd:toggle-panel", group: "Command", label: "Toggle Panel", hint: "⌘J", run: togglePanel },
      {
        id: "cmd:merge-queue",
        group: "Command",
        label: "Merge Queue",
        run: () => {
          setPanelOpen(true);
          setPanelTab("queue");
        },
      },
      { id: "cmd:close-tab", group: "Command", label: "Close Tab", hint: "⌘W", run: () => dispatch({ type: "closeActive" }) },
    ];
    if (!paletteOpen) return commands;
    const titleByRoot = new Map((tasks ?? []).map((t) => [t.worktree_path, t.title]));
    const files: PaletteItem[] = [];
    for (const [key, entries] of Object.entries(dirs.entries)) {
      const root = key.slice(0, key.indexOf("\0"));
      if (root !== workspace && !titleByRoot.has(root)) continue;
      const scopeLabel = root === workspace ? baseName(root) : titleByRoot.get(root);
      for (const entry of entries) {
        if (entry.is_dir) continue;
        files.push({
          id: `file:${dirKey(root, entry.rel_path)}`,
          group: "File",
          label: entry.name,
          detail: `${entry.rel_path} · ${scopeLabel}`,
          run: () => openFile(root, entry.rel_path),
        });
      }
    }
    return commands.concat(files);
  }, [paletteOpen, dirs.entries, tasks, workspace, openFolder, newAgent, toggleSidebar, showFiles, showAgents, togglePanel, openFile]);

  const agentStage = selectedTaskId ? (
    <AgentView
      active={activeKey === null}
      taskId={selectedTaskId}
      task={selectedTask}
      mergeQueueRunning={selectedTask ? mergeQueues.isRunning(selectedTask.repo_path) : false}
      onStop={handleStop}
      onDiscard={handleDiscard}
      onReveal={handleReveal}
      onOpenDiff={openDiff}
      onContinue={handleContinue}
      onCommit={handleCommit}
      onMerge={handleMerge}
      onSelectTask={selectTask}
    />
  ) : null;

  const welcome = (
    <div className="welcome" role="region" aria-label="Welcome">
      <Mark />
      <h2>Lore</h2>
      <p>Run coding agents in parallel, each in its own git worktree and branch. Your checkout is not touched.</p>
      <div className="welcome__actions">
        <button type="button" className="wb-btn wb-btn--primary" onClick={() => void openFolder()}>
          Open Folder…
        </button>
        <button type="button" className="wb-btn" onClick={newAgent} disabled={!workspace}>
          New agent
        </button>
      </div>
      <dl className="welcome__keys">
        <div>
          <dt>Command palette</dt>
          <dd>
            <kbd>⌘K</kbd>
          </dd>
        </div>
        <div>
          <dt>Toggle sidebar</dt>
          <dd>
            <kbd>⌘B</kbd>
          </dd>
        </div>
        <div>
          <dt>Toggle panel</dt>
          <dd>
            <kbd>⌘J</kbd>
          </dd>
        </div>
        <div>
          <dt>Close tab</dt>
          <dd>
            <kbd>⌘W</kbd>
          </dd>
        </div>
      </dl>
    </div>
  );

  return (
    <div className="wb">
      <header className="wb-titlebar">
        <div className="wb-titlebar__left">
          <TitlebarLights />
          <Mark />
          <h1 className="wb-titlebar__workspace visually-hidden" title={workspace ?? undefined}>
            {workspace ? baseName(workspace) : "No folder open"}
          </h1>
        </div>
        <button
          type="button"
          className="wb-titlebar__search"
          aria-label="Search"
          title="Search (⌘K)"
          onClick={() => setPaletteOpen(true)}
        >
          <SearchIcon />
          <span className="wb-titlebar__search-label">{workspace ? baseName(workspace) : "Search"}</span>
          <kbd>⌘K</kbd>
        </button>
        <div className="wb-titlebar__right">
          <button
            type="button"
            className={`wb-icon-btn${sidebarOpen ? " wb-icon-btn--on" : ""}`}
            aria-label="Toggle sidebar"
            aria-pressed={sidebarOpen}
            title="Toggle sidebar (⌘B)"
            onClick={toggleSidebar}
          >
            <SidebarIcon />
          </button>
          <button
            type="button"
            className={`wb-icon-btn${panelOpen ? " wb-icon-btn--on" : ""}`}
            aria-label="Toggle panel"
            aria-pressed={panelOpen}
            title="Toggle panel (⌘J)"
            onClick={togglePanel}
          >
            <PanelIcon />
          </button>
          <button type="button" className="wb-btn wb-btn--small" onClick={() => void openFolder()}>
            Open Folder…
          </button>
        </div>
      </header>
      {workspaceError && (
        <div className="wb-banner" role="alert">
          <span className="wb-banner__text">{workspaceError}</span>
          <button type="button" className="wb-banner__dismiss" onClick={() => setWorkspaceError(null)}>
            Dismiss
          </button>
        </div>
      )}

      <div className="wb-main">
        {sidebarOpen && (
          <>
            <aside className="wb-sidebar" style={{ width: sidebarWidth }} aria-label="Sidebar">
              <div className="wb-sidebar__switch" role="tablist" aria-label="Sidebar views">
                <button
                  type="button"
                  role="tab"
                  className={`wb-sidebar__tab${sidebarView === "agents" ? " wb-sidebar__tab--on" : ""}`}
                  aria-selected={sidebarView === "agents"}
                  onClick={showAgents}
                >
                  Agents
                </button>
                <button
                  type="button"
                  role="tab"
                  className={`wb-sidebar__tab${sidebarView === "explorer" ? " wb-sidebar__tab--on" : ""}`}
                  aria-selected={sidebarView === "explorer"}
                  onClick={showFiles}
                >
                  Files
                </button>
              </div>
              {sidebarView === "agents" ? (
                <AgentsPanel
                  workspace={workspace}
                  tasks={visibleTasks}
                  allRepos={allRepos}
                  onAllReposChange={setAllRepos}
                  hiddenCount={hiddenCount}
                  listError={listError}
                  loadWarning={loadWarning}
                  selectedTaskId={selectedTaskId}
                  onSelect={selectTask}
                  onStop={handleStop}
                  onDiscard={handleDiscard}
                  onReveal={handleReveal}
                  onOpenDiff={openDiff}
                  onCreated={handleCreated}
                  onOpenFolder={() => void openFolder()}
                  focusToken={newAgentToken}
                />
              ) : (
                <ExplorerPanel
                  workspace={workspace}
                  tasks={visibleTasks}
                  scope={scope}
                  onScopeChange={setScope}
                  root={scopeRoot}
                  cache={dirs}
                  onOpenFile={openFile}
                  onOpenFolder={() => void openFolder()}
                  activeFile={activeFile}
                />
              )}
            </aside>
            <Sash
              orientation="vertical"
              label="Resize sidebar"
              value={sidebarWidth}
              min={SIDEBAR_MIN}
              max={SIDEBAR_MAX}
              direction={1}
              onChange={setSidebarWidth}
            />
          </>
        )}
        <div className="wb-center">
          <main className="wb-editor" aria-label="Editor">
            <EditorTabs
              tabs={tabs}
              activeKey={activeKey}
              taskTitle={taskTitle}
              onActivate={(key) => dispatch({ type: "activate", key })}
              onClose={(key) => dispatch({ type: "close", key })}
              renderTab={renderTab}
              stage={agentStage ?? welcome}
            />
          </main>
          {panelOpen && (
            <>
              <Sash
                orientation="horizontal"
                label="Resize panel"
                value={panelHeight}
                min={PANEL_MIN}
                max={PANEL_MAX}
                direction={-1}
                onChange={setPanelHeight}
              />
              <BottomPanel
                height={panelHeight}
                tab={panelTab}
                onTabChange={setPanelTab}
                onClose={() => setPanelOpen(false)}
                task={selectedTask}
                repoPath={workspace}
                tasks={tasks}
                mergeQueues={mergeQueues}
                onOpenChange={(task, rel) => openFile(task.worktree_path, rel)}
              />
            </>
          )}
        </div>
      </div>

      <StatusBar
        workspace={workspace}
        runningCount={runningCount}
        attentionCount={attentionCount}
        selectedBranch={selectedTask?.branch ?? null}
        onShowAgents={showAgents}
      />

      {paletteOpen && <CommandPalette items={paletteItems} onClose={() => setPaletteOpen(false)} />}
    </div>
  );
}

function TitlebarLights() {
  const native = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  return (
    <span className="wb-titlebar__lights" aria-hidden="true">
      {!native && (
        <>
          <i className="wb-titlebar__light wb-titlebar__light--close" />
          <i className="wb-titlebar__light wb-titlebar__light--min" />
          <i className="wb-titlebar__light wb-titlebar__light--zoom" />
        </>
      )}
    </span>
  );
}
