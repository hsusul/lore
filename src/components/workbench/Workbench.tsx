import { useCallback, useEffect, useMemo, useReducer, useRef, useState, type CSSProperties } from "react";
import { Toaster, toast } from "sonner";

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
import AgentView, { type AgentPane } from "./AgentView";
import BottomPanel, { type PanelTab } from "./BottomPanel";
import CommandPalette, { type PaletteItem } from "./CommandPalette";
import EditorTabs from "./EditorTabs";
import FileView from "./FileView";
import Home from "./Home";
import NewAgentDialog from "./NewAgentDialog";
import { Mark, PanelIcon, SearchIcon, SidebarIcon } from "./icons";
import Sash from "./Sash";
import { ExplorerPanel } from "./Sidebar";
import SidebarNav from "./SidebarNav";
import { needsUser } from "./board";
import {
  AGENT_LABELS,
  PANEL_MAX,
  PANEL_MIN,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  STORAGE_KEYS,
  baseName,
  errorText,
  fileTab,
  initialTabs,
  readStored,
  readStoredNumber,
  shortcut,
  stateLabel,
  tabsReducer,
  writeStored,
  type SidebarView,
  type Tab,
} from "./state";
import StatusBar from "./StatusBar";
import { dirKey, useDirCache } from "./useDirCache";
import { appInFront, eventHeadline, notify, setAttentionBadge, taskEvents } from "./notify";
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
  const [sidebarOpen, setSidebarOpen] = useState(() => readStored(STORAGE_KEYS.sidebarOpen) !== "false");
  const [sidebarView, setSidebarView] = useState<SidebarView>(() =>
    readStored(STORAGE_KEYS.sidebarView) === "explorer" ? "explorer" : "agents",
  );
  const [panelOpen, setPanelOpen] = useState(() => readStored(STORAGE_KEYS.panelOpen) === "true");
  const [panelHeight, setPanelHeight] = useState(() =>
    readStoredNumber(STORAGE_KEYS.panelHeight, 280, PANEL_MIN, PANEL_MAX),
  );
  const [panelTab, setPanelTab] = useState<PanelTab>("output");
  const [allRepos, setAllRepos] = useState(() => readStored(STORAGE_KEYS.allRepos) === "true");
  const [scope, setScope] = useState("repo");
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [agentPane, setAgentPane] = useState<AgentPane>("activity");
  const [paletteOpen, setPaletteOpen] = useState(false);
  /** The New agent dialog: closed, or open with an optional starting prompt. */
  const [newAgentDialog, setNewAgentDialog] = useState<{ draft?: string } | null>(null);
  const dialogOpenRef = useRef(false);
  dialogOpenRef.current = newAgentDialog !== null;

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
  useEffect(() => writeStored(STORAGE_KEYS.sidebarOpen, String(sidebarOpen)), [sidebarOpen]);
  useEffect(() => writeStored(STORAGE_KEYS.sidebarView, sidebarView), [sidebarView]);
  useEffect(() => writeStored(STORAGE_KEYS.allRepos, String(allRepos)), [allRepos]);

  const taskById = useMemo(() => new Map((tasks ?? []).map((t) => [t.id, t])), [tasks]);
  const visibleTasks = useMemo(() => {
    if (tasks === null) return null;
    if (allRepos || workspace === null) return tasks;
    return tasks.filter((t) => t.repo_path === workspace);
  }, [tasks, allRepos, workspace]);
  const hiddenCount = (tasks?.length ?? 0) - (visibleTasks?.length ?? 0);
  const runningCount = (visibleTasks ?? []).filter((t) => t.state === "running").length;
  const attentionCount = (visibleTasks ?? []).filter(needsUser).length;
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

  const selectedRef = useRef(selectedTaskId);
  selectedRef.current = selectedTaskId;

  const selectTask = useCallback((id: string) => {
    // Another agent starts on its activity; re-selecting keeps the current view.
    if (selectedRef.current !== id) setAgentPane("activity");
    setSelectedTaskId(id);
    dispatch({ type: "clearActive" });
  }, []);

  /** The agent's Changes view, next to its commit and merge actions. */
  const openDiff = useCallback((id: string) => {
    setSelectedTaskId(id);
    setAgentPane("changes");
    dispatch({ type: "clearActive" });
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
      setNewAgentDialog(null);
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

  /** Back to the overview: no agent selected and no file or diff in front. */
  const showOverview = useCallback(() => {
    setSelectedTaskId(null);
    dispatch({ type: "clearActive" });
  }, []);

  const workspaceRef = useRef(workspace);
  workspaceRef.current = workspace;

  /**
   * Open the New agent dialog over whatever is on screen, optionally starting
   * from `draft`. Without an open folder there is nowhere to launch, so it asks
   * for one instead.
   */
  const newAgent = useCallback(
    (draft?: string) => {
      if (!workspaceRef.current) {
        void openFolder();
        return;
      }
      setNewAgentDialog({ draft });
    },
    [openFolder],
  );

  // Tell the user when an agent finishes, fails, needs them, or lands: a toast
  // while Lore is in front (except for the agent already on screen), an OS
  // notification while it is not.
  const previousTasks = useRef<TaskDto[] | null>(null);
  useEffect(() => {
    const events = taskEvents(previousTasks.current, tasks);
    if (tasks !== null) previousTasks.current = tasks;
    const front = appInFront();
    for (const event of events) {
      const headline = eventHeadline(event);
      if (!front) {
        void notify(headline, event.message);
        continue;
      }
      if (event.id === selectedRef.current) continue;
      const show =
        event.kind === "failed"
          ? toast.error
          : event.kind === "attention"
            ? toast.warning
            : event.kind === "handoff"
              ? toast.info
              : toast.success;
      show(headline, {
        id: `${event.id}:${event.kind}`,
        description: event.message,
        action: { label: "Open", onClick: () => selectTask(event.id) },
      });
    }
  }, [tasks, selectTask]);

  // The dock badge counts every repository's agents that need the user.
  const needYou = useMemo(() => (tasks ?? []).filter(needsUser).length, [tasks]);
  useEffect(() => setAttentionBadge(needYou), [needYou]);

  const openMergeQueue = useCallback(() => {
    setPanelOpen(true);
    setPanelTab("queue");
  }, []);

  const visibleTasksRef = useRef(visibleTasks);
  visibleTasksRef.current = visibleTasks;

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      // The New agent dialog is modal: shortcuts wait until it closes.
      if (dialogOpenRef.current) return;
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
        case "n":
          e.preventDefault();
          newAgent();
          break;
        default: {
          // ⌘1–⌘9 open the agents in sidebar order.
          const n = Number(e.key);
          const target = Number.isInteger(n) && n >= 1 ? visibleTasksRef.current?.[n - 1] : undefined;
          if (target) {
            e.preventDefault();
            selectTask(target.id);
          }
        }
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleSidebar, togglePanel, newAgent, selectTask]);

  const taskTitle = useCallback((id: string) => taskById.get(id)?.title ?? "Discarded agent", [taskById]);

  const renderTab = (tab: Tab, active: boolean) => {
    switch (tab.kind) {
      case "file":
        return <FileView root={tab.root} relPath={tab.relPath} />;
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
      { id: "cmd:open-folder", group: "Command", label: "Open folder…", run: () => void openFolder() },
      { id: "cmd:new-agent", group: "Command", label: "New agent", hint: shortcut("N"), run: () => newAgent() },
      { id: "cmd:overview", group: "Command", label: "Go to overview", run: showOverview },
      { id: "cmd:toggle-sidebar", group: "Command", label: "Toggle sidebar", hint: shortcut("B"), run: toggleSidebar },
      { id: "cmd:show-files", group: "Command", label: "Show files", run: showFiles },
      { id: "cmd:show-agents", group: "Command", label: "Show agents", run: showAgents },
      { id: "cmd:toggle-panel", group: "Command", label: "Toggle panel", hint: shortcut("J"), run: togglePanel },
      { id: "cmd:merge-queue", group: "Command", label: "Merge queue", run: openMergeQueue },
      {
        id: "cmd:close-tab",
        group: "Command",
        label: "Close tab",
        hint: shortcut("W"),
        run: () => dispatch({ type: "closeActive" }),
      },
    ];
    if (!paletteOpen) return commands;
    const chosen = selectedTaskId ? taskById.get(selectedTaskId) : undefined;
    if (chosen) {
      const reportError = (e: unknown) => setWorkspaceError(errorText(e));
      commands.push(
        {
          id: "cmd:agent-changes",
          group: "Command",
          label: `Show changes: ${chosen.title}`,
          run: () => openDiff(chosen.id),
        },
        {
          id: "cmd:agent-activity",
          group: "Command",
          label: `Show activity: ${chosen.title}`,
          run: () => {
            selectTask(chosen.id);
            setAgentPane("activity");
          },
        },
        {
          id: "cmd:agent-reveal",
          group: "Command",
          label: `Reveal worktree: ${chosen.title}`,
          run: () => void handleReveal(chosen.id).catch(reportError),
        },
      );
      if (chosen.state === "running") {
        commands.push({
          id: "cmd:agent-stop",
          group: "Command",
          label: `Stop agent: ${chosen.title}`,
          run: () => void handleStop(chosen.id).catch(reportError),
        });
      }
    }
    const agents: PaletteItem[] = (visibleTasks ?? []).map((t, i) => ({
      id: `agent:${t.id}`,
      group: "Agent",
      label: t.title,
      detail: `${t.merged_into ? "Merged" : stateLabel(t)} · ${AGENT_LABELS[t.agent]} · ${t.branch}`,
      hint: needsUser(t) ? "needs you" : i < 9 ? shortcut(String(i + 1)) : "agent",
      run: () => selectTask(t.id),
    }));
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
    return commands.concat(agents, files);
  }, [
    paletteOpen,
    dirs.entries,
    tasks,
    visibleTasks,
    taskById,
    selectedTaskId,
    selectTask,
    openDiff,
    handleStop,
    handleReveal,
    workspace,
    openFolder,
    newAgent,
    showOverview,
    openMergeQueue,
    toggleSidebar,
    showFiles,
    showAgents,
    togglePanel,
    openFile,
  ]);

  const agentStage = selectedTaskId ? (
    <AgentView
      active={activeKey === null}
      taskId={selectedTaskId}
      task={selectedTask}
      mergeQueueRunning={selectedTask ? mergeQueues.isRunning(selectedTask.repo_path) : false}
      onStop={handleStop}
      onDiscard={handleDiscard}
      onReveal={handleReveal}
      onContinue={handleContinue}
      onCommit={handleCommit}
      onMerge={handleMerge}
      onSelectTask={selectTask}
      pane={agentPane}
      onPaneChange={setAgentPane}
      onOpenFile={openFile}
    />
  ) : null;

  const sidebarSwitch = (
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
  );

  const home = (
    <Home
      workspace={workspace}
      tasks={visibleTasks}
      onSelect={selectTask}
      onNewAgent={() => newAgent()}
      onOpenFolder={() => void openFolder()}
      onOpenMergeQueue={openMergeQueue}
    />
  );

  return (
    <div className="wb">
      <header className="wb-titlebar">
        <div className="wb-titlebar__left">
          <TitlebarLights />
          <button type="button" className="wb-titlebar__home" aria-label="Overview" title="Overview" onClick={showOverview}>
            <Mark />
          </button>
          <h1 className="wb-titlebar__workspace visually-hidden" title={workspace ?? undefined}>
            {workspace ? baseName(workspace) : "No folder open"}
          </h1>
        </div>
        <button
          type="button"
          className="wb-titlebar__search"
          aria-label="Search"
          title={`Search (${shortcut("K")})`}
          onClick={() => setPaletteOpen(true)}
        >
          <SearchIcon />
          <span className="wb-titlebar__search-label">
            {workspace ? `Search ${baseName(workspace)}` : "Search"}
          </span>
          <kbd>{shortcut("K")}</kbd>
        </button>
        <div className="wb-titlebar__right">
          <button
            type="button"
            className={`wb-icon-btn${sidebarOpen ? " wb-icon-btn--on" : ""}`}
            aria-label="Toggle sidebar"
            aria-pressed={sidebarOpen}
            title={`Toggle sidebar (${shortcut("B")})`}
            onClick={toggleSidebar}
          >
            <SidebarIcon />
          </button>
          <button
            type="button"
            className={`wb-icon-btn${panelOpen ? " wb-icon-btn--on" : ""}`}
            aria-label="Toggle panel"
            aria-pressed={panelOpen}
            title={`Toggle panel (${shortcut("J")})`}
            onClick={togglePanel}
          >
            <PanelIcon />
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
              <SidebarNav
                workspace={workspace}
                onOpenFolder={() => void openFolder()}
                onNewAgent={() => newAgent()}
                onShowOverview={showOverview}
                overviewActive={selectedTaskId === null && activeKey === null}
                attentionCount={attentionCount}
              />
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
                  switcher={sidebarSwitch}
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
                  switcher={sidebarSwitch}
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
              stage={agentStage ?? home}
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
        onShowOverview={showOverview}
      />

      <Toaster
        theme="dark"
        position="bottom-right"
        offset={{ bottom: 34, right: 12 }}
        visibleToasts={4}
        style={TOAST_THEME}
      />

      {newAgentDialog && workspace && (
        <NewAgentDialog
          workspace={workspace}
          baseBranch={(tasks ?? []).find((t) => t.repo_path === workspace && t.repo_branch)?.repo_branch}
          draft={newAgentDialog.draft}
          onClose={() => setNewAgentDialog(null)}
          onCreated={handleCreated}
          onOpenFolder={() => void openFolder()}
        />
      )}

      {paletteOpen && (
        <CommandPalette
          items={paletteItems}
          onClose={() => setPaletteOpen(false)}
          onNewAgent={workspace ? (prompt) => newAgent(prompt) : undefined}
        />
      )}
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

/** Sonner's colours from the workbench tokens. */
const TOAST_THEME = {
  "--normal-bg": "var(--bg-overlay)",
  "--normal-border": "var(--line-strong)",
  "--normal-text": "var(--text-1)",
  "--border-radius": "var(--r-lg)",
  fontFamily: "var(--font-ui)",
} as CSSProperties;
