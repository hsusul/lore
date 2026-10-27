import { clsx } from "clsx";
import { Command as CommandIcon } from "lucide-react";
import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";

import CommandPalette, { type Command } from "./components/CommandPalette";
import { BrowsePane, groupHits, ScopeBar, SearchPane } from "./components/ListPane";
import type { OpenVia } from "./components/ListBox";
import MessageBar from "./components/MessageBar";
import { ConfirmDialog, NameDialog } from "./components/Modal";
import Onboarding from "./components/Onboarding";
import Reader from "./components/Reader";
import ReaderToolbar, { type ActionNote, type ReaderActions } from "./components/ReaderToolbar";
import ScanControl from "./components/ScanControl";
import SearchField, { type FilterValues } from "./components/SearchField";
import SettingsDialog from "./components/SettingsDialog";
import Sidebar, { SidebarToggle } from "./components/Sidebar";
import { sourceRow, sourceSections } from "./components/sources";
import Toolbar, { ScopeMenu, ScopeTitle } from "./components/Toolbar";
import { Button, ICON } from "./components/ui/button";
import { Tooltip, TooltipProvider } from "./components/ui/tooltip";
import { agentLabel } from "./format";
import { isRunnable, queryFilters, queryTerms, withoutFilters } from "./hooks/query";
import { useCatalog, type Catalog } from "./hooks/useCatalog";
import { useDebounced } from "./hooks/useDebounced";
import { useLastScan } from "./hooks/useLastScan";
import { useNote } from "./hooks/useNote";
import { useOpenSession } from "./hooks/useOpenSession";
import { usePagedList, type FetchPage } from "./hooks/usePagedList";
import { useSidebar } from "./hooks/useSidebar";
import { useToolCallMode } from "./hooks/useToolCallMode";
import { ALL, currentSource, initialWorkspace, workspaceReducer, type Overlay, type Source } from "./hooks/workspace";
import {
  addAgentRoot,
  chooseAgentRootDirectory,
  createFolder,
  deleteFolder,
  exportSessionMarkdown,
  forgetEverything,
  forgetSession,
  listFolderSessionsPage,
  listRepositorySessionsPage,
  listSessionsPage,
  onScanProgress,
  removeAgentRoot,
  renameFolder,
  rescan,
  saveSessionExport,
  searchPage,
  setSessionFolder,
  type DetectedAgent,
  type FolderSummary,
  type SearchHit,
  type SearchSort,
  type SessionSummary,
} from "./ipc";

const SESSION_PAGE = 100;
const SEARCH_PAGE = 50;
const SEARCH_DEBOUNCE_MS = 180;
const EMPTY: Catalog = { agents: [], repositories: [], folders: [] };

const sessionKey = (s: SessionSummary) => s.id;
const hitKey = (h: SearchHit) => `${h.source_kind}:${h.source_id}:${h.field}`;

function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return !!el && (el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName));
}

/** The reader action a ⌘ shortcut names (spec 11): ⇧⌘C, ⌘S, ⌘⌫. ⌘⌫ deletes text in a field, so it stays there. */
function readerKey(event: KeyboardEvent): "copy" | "save" | "forget" | null {
  const key = event.key.toLowerCase();
  if (event.shiftKey) return key === "c" ? "copy" : null;
  if (key === "s") return "save";
  if (event.key === "Backspace" && !isTyping(event.target)) return "forget";
  return null;
}

export default function App() {
  const [ws, dispatch] = useReducer(workspaceReducer, initialWorkspace);
  const catalog = useCatalog();
  const { agents, repositories, folders } = catalog.state.status === "ready" ? catalog.state.catalog : EMPTY;
  const [busyAgent, setBusyAgent] = useState<string | null>(null);
  const sidebar = useSidebar();
  const lastScanAt = useLastScan(ws.scan);
  const [toolCalls, setToolCalls] = useToolCallMode();
  const [actionNote, showNote] = useNote<ActionNote>();

  const queryRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const readerRef = useRef<HTMLElement>(null);
  const focusReaderOnLoad = useRef(false);

  const notice = useCallback((text: string) => dispatch({ type: "flash", flash: { tone: "notice", text } }), []);
  const fail = useCallback((e: unknown) => dispatch({ type: "flash", flash: { tone: "error", text: String(e) } }), []);
  const overlay = useCallback((next: Overlay) => dispatch({ type: "overlay", overlay: next }), []);
  const closeOverlay = useCallback(() => overlay({ kind: "none" }), [overlay]);

  const scope = ws.scope;
  const fetchBrowse = useMemo<FetchPage<SessionSummary>>(
    () => async (limit, cursor) => {
      const page =
        scope.kind === "folder"
          ? await listFolderSessionsPage(scope.id, limit, cursor)
          : scope.kind === "repository"
            ? await listRepositorySessionsPage(scope.id, limit, cursor)
            : await listSessionsPage(limit, cursor);
      return { items: page.sessions, cursor: page.next_cursor };
    },
    [scope],
  );
  const browse = usePagedList(fetchBrowse, SESSION_PAGE, sessionKey);

  // A bare `agent:` waiting for a value searches nothing yet, so the list stays put.
  const searching = isRunnable(ws.query);
  const settledQuery = useDebounced(ws.query, SEARCH_DEBOUNCE_MS);
  const terms = useMemo(() => queryTerms(settledQuery), [settledQuery]);
  // The backend scopes a search to a repository by id; a folder cannot narrow
  // a search yet, so the scope bar says so rather than pretending.
  const backendQuery = scope.kind === "repository" ? `${settledQuery.trim()} repo:${scope.id}` : settledQuery.trim();
  const hasQuery = isRunnable(settledQuery);
  const fetchSearch = useMemo<FetchPage<SearchHit> | null>(
    () =>
      !hasQuery
        ? null
        : async (limit, cursor) => {
            const page = await searchPage(backendQuery, limit, cursor, ws.sort);
            return { items: page.hits, cursor: page.next_cursor };
          },
    [backendQuery, hasQuery, ws.sort],
  );
  const search = usePagedList(fetchSearch, SEARCH_PAGE, hitKey);
  const searchPending = ws.query !== settledQuery;

  const open = useOpenSession(ws.openId);
  const openState = open.state;
  const reading = useMemo(
    () => (openState.status === "ready" ? { id: openState.id, title: openState.session.detail.summary.title ?? "Untitled session" } : null),
    [openState],
  );

  const refreshRef = useRef<() => Promise<void>>(async () => {});
  const refreshAll = useCallback(async () => {
    try {
      await Promise.all([catalog.refresh(), browse.refresh()]);
    } catch (e) {
      fail(e);
    }
  }, [browse, catalog, fail]);
  useEffect(() => {
    refreshRef.current = refreshAll;
  }, [refreshAll]);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const unlisten = onScanProgress((progress) => {
      dispatch({ type: "scan-progress", progress });
      // Coalesce a storm of progress events into one archive refresh, so new
      // sessions appear while the worker commits them.
      if (progress.done || progress.ingested + progress.skipped + progress.failed > 0) {
        if (timer !== null) clearTimeout(timer);
        timer = setTimeout(() => void refreshRef.current(), 300);
      }
    });
    return () => {
      if (timer !== null) clearTimeout(timer);
      void unlisten.then((off) => off());
    };
  }, []);

  const focusQuery = useCallback(() => queryRef.current?.focus(), []);
  const focusList = useCallback(() => listRef.current?.focus(), []);

  // Arrow keys open sessions in place, so Enter often lands on a session that is
  // already open; the request count lets that Enter still move focus.
  const [focusRequests, setFocusRequests] = useState(0);
  const openSession = useCallback((id: string, via: OpenVia = "pointer") => {
    focusReaderOnLoad.current = via === "keyboard";
    if (via === "keyboard") setFocusRequests((n) => n + 1);
    dispatch({ type: "open", id });
  }, []);

  // Launch opens the most recent session, with the selection in the list (spec 10).
  const launched = useRef(false);
  const browseReady = browse.state.status === "ready" ? browse.state.items : null;
  useEffect(() => {
    if (launched.current || browseReady === null) return;
    launched.current = true;
    if (browseReady.length === 0 || ws.openId !== null || isRunnable(ws.query) || ws.scope.kind !== "all") return;
    dispatch({ type: "open", id: browseReady[0].id });
    if (document.activeElement === document.body) listRef.current?.focus();
  }, [browseReady, ws.openId, ws.query, ws.scope.kind]);

  useEffect(() => {
    if (open.state.status === "ready" && open.state.id === ws.openId && focusReaderOnLoad.current) {
      focusReaderOnLoad.current = false;
      readerRef.current?.focus();
    }
  }, [open.state, ws.openId, focusRequests]);

  const chooseSource = useCallback((source: Source, focus: "list" | "stay" = "list") => {
    dispatch({ type: "source", source });
    if (focus === "list") requestAnimationFrame(() => listRef.current?.focus());
  }, []);

  const startScan = useCallback(async () => {
    dispatch({ type: "scan-requested" });
    try {
      await rescan();
    } catch (e) {
      dispatch({ type: "scan-failed", error: String(e) });
    }
  }, []);

  const addRoot = useCallback(
    async (agent: DetectedAgent) => {
      setBusyAgent(agent.id);
      try {
        const path = await chooseAgentRootDirectory(agent.display_name);
        if (path === null) return;
        await addAgentRoot(agent.id, path);
        notice(`Added ${path}. Lore is scanning it now.`);
        await refreshAll();
      } catch (e) {
        fail(e);
      } finally {
        setBusyAgent(null);
      }
    },
    [fail, notice, refreshAll],
  );

  const removeRoot = useCallback(
    async (agent: DetectedAgent, path: string) => {
      setBusyAgent(agent.id);
      try {
        await removeAgentRoot(agent.id, path);
        notice(`Stopped reading ${path}. Sessions already archived stay.`);
        await refreshAll();
      } catch (e) {
        fail(e);
      } finally {
        setBusyAgent(null);
      }
    },
    [fail, notice, refreshAll],
  );

  const newFolder = useCallback(
    () =>
      overlay({
        kind: "name",
        title: "New Folder",
        label: "Folder name",
        initial: "",
        confirmLabel: "Create folder",
        onSubmit: (name) =>
          void createFolder(name).then(
            async (folder) => {
              await refreshAll();
              notice(`Created folder “${folder.name}”.`);
            },
            fail,
          ),
      }),
    [fail, notice, overlay, refreshAll],
  );

  const forgetAll = useCallback(
    () =>
      overlay({
        kind: "confirm",
        title: "Forget everything?",
        body: "This removes every session, repository, finding, and backup from Lore. Your original agent logs are not touched. This cannot be undone.",
        confirmLabel: "Forget everything",
        onConfirm: () =>
          void forgetEverything().then(async (report) => {
            dispatch({ type: "forgot-everything" });
            await refreshAll();
            notice(`Archive cleared. ${report.blobs_removed} stored blobs removed.`);
          }, fail),
      }),
    [fail, notice, overlay, refreshAll],
  );

  const currentFolder = scope.kind === "folder" ? folders.find((f) => f.id === scope.id) ?? null : null;
  const currentRepo = scope.kind === "repository" ? repositories.find((r) => r.id === scope.id) ?? null : null;
  const sections = useMemo(() => sourceSections({ agents, repositories, folders }), [agents, folders, repositories]);
  const selected = currentSource(ws);
  const selectedRow = sourceRow(sections, selected);
  const scopeLabel = selectedRow?.label ?? "All Sessions";

  const renameFolderNamed = (folder: FolderSummary) =>
    overlay({
      kind: "name",
      title: "Rename Folder",
      label: "Folder name",
      initial: folder.name,
      confirmLabel: "Rename",
      onSubmit: (name) => void renameFolder(folder.id, name).then(refreshAll, fail),
    });

  const deleteFolderNamed = (folder: FolderSummary) =>
    overlay({
      kind: "confirm",
      title: `Delete “${folder.name}”?`,
      body: "The folder goes away. Its sessions stay in Lore, just not filed.",
      confirmLabel: "Delete Folder",
      onConfirm: () =>
        void deleteFolder(folder.id).then(async () => {
          if (scope.kind === "folder" && scope.id === folder.id) dispatch({ type: "source", source: ALL });
          await refreshAll();
          notice(`Deleted folder “${folder.name}”.`);
        }, fail),
    });
  const folderActions = { onNewFolder: newFolder, onRenameFolder: renameFolderNamed, onDeleteFolder: deleteFolderNamed };

  // Reader actions confirm on their own controls (spec 9.12): Copy and File flip their
  // tooltips, Save answers with the system save panel, Forget removes the row. Errors still
  // go to the notices line.
  const actions: ReaderActions = {
    copyMarkdown: (id) =>
      void (async () => {
        try {
          const markdown = await exportSessionMarkdown(id, false);
          if (markdown == null) return;
          if (!navigator.clipboard?.writeText) {
            fail("The clipboard is not available here. Use Save as File instead.");
            return;
          }
          await navigator.clipboard.writeText(markdown);
          showNote({ control: "copy", text: "Copied" });
        } catch (e) {
          fail(e);
        }
      })(),
    saveFile: (id) => void saveSessionExport(id, null, false).catch(fail),
    file: (id, folderId) =>
      void setSessionFolder(id, folderId).then(async () => {
        await refreshAll();
        const name = folders.find((f) => f.id === folderId)?.name;
        showNote({ control: "file", text: name ? `Filed in “${name}”` : "Removed from Folder" });
      }, fail),
    forget: (id, title) =>
      overlay({
        kind: "confirm",
        title: `Forget “${title}”?`,
        body: "Lore removes this session and everything derived from it. The original agent log stays on disk.",
        confirmLabel: "Forget Session",
        onConfirm: () =>
          void forgetSession(id).then(async () => {
            dispatch({ type: "open", id: null });
            await refreshAll();
          }, fail),
      }),
  };
  const actionsRef = useRef(actions);
  actionsRef.current = actions;

  const searchArchive = useCallback(async (q: string): Promise<Command[]> => {
    const page = await searchPage(q, SEARCH_PAGE);
    const seen = new Set<string>();
    return page.hits
      .filter((hit) => !seen.has(hit.session_id) && !!seen.add(hit.session_id))
      .map((hit) => ({
        id: `cmd-session-${hit.session_id}`,
        group: "Archive",
        label: hit.title ?? "Untitled session",
        hint: `${agentLabel(hit.agent_id)}, full-text match`,
        run: () => openSession(hit.session_id),
      }));
  }, [openSession]);

  const toggleSidebar = sidebar.toggle;
  const scanPhase = ws.scan.phase;
  const browseState = browse.state;
  const commands = useMemo<Command[]>(
    () => [
      { id: "cmd-rescan", group: "Action", label: "Rescan archive", hint: "⌘R", run: () => void startScan() },
      { id: "cmd-search", group: "Action", label: "Search sessions", hint: "/", run: () => requestAnimationFrame(focusQuery) },
      { id: "cmd-all", group: "Action", label: "Browse all sessions", run: () => chooseSource(ALL) },
      { id: "cmd-folder", group: "Action", label: "New folder…", run: newFolder },
      { id: "cmd-sidebar", group: "Action", label: "Show or hide the sidebar", hint: "⌃⌘S", run: sidebar.toggle },
      { id: "cmd-settings", group: "Action", label: "Settings", hint: "Sources, backups, privacy", run: () => overlay({ kind: "settings" }) },
      ...repositories.map((r) => ({
        id: `cmd-repo-${r.id}`,
        group: "Repository",
        label: r.display_name,
        hint: `${r.session_count} sessions`,
        run: () => chooseSource({ kind: "repository", id: r.id }),
      })),
      ...folders.map((f) => ({
        id: `cmd-folder-${f.id}`,
        group: "Folder",
        label: f.name,
        hint: `${f.session_count} sessions`,
        run: () => chooseSource({ kind: "folder", id: f.id }),
      })),
      ...(browseState.status === "ready" ? browseState.items : []).map((s) => ({
        id: `cmd-session-${s.id}`,
        group: "Session",
        label: s.title ?? "Untitled session",
        hint: agentLabel(s.agent_id),
        run: () => openSession(s.id),
      })),
    ],
    [browseState, chooseSource, focusQuery, folders, newFolder, openSession, overlay, repositories, sidebar.toggle, startScan],
  );
  const readerCommands: Command[] = [
    {
      id: "cmd-tool-calls",
      group: "Action",
      label: toolCalls === "collapsed" ? "Show Tool Calls Expanded" : "Show Tool Calls Collapsed",
      run: () => setToolCalls(toolCalls === "collapsed" ? "expanded" : "collapsed"),
    },
    ...(reading
      ? [
          { id: "cmd-copy", group: "Action", label: "Copy as Markdown", hint: "⇧⌘C", run: () => actions.copyMarkdown(reading.id) },
          { id: "cmd-save", group: "Action", label: "Save as File…", hint: "⌘S", run: () => actions.saveFile(reading.id) },
          ...folders.map((f) => ({
            id: `cmd-file-${f.id}`,
            group: "Action",
            label: `File in “${f.name}”`,
            run: () => actions.file(reading.id, f.id),
          })),
          { id: "cmd-unfile", group: "Action", label: "Remove from Folder", run: () => actions.file(reading.id, null) },
          { id: "cmd-forget", group: "Action", label: "Forget Session…", hint: "⌘⌫", run: () => actions.forget(reading.id, reading.title) },
        ]
      : []),
  ];
  const paletteCommands: Command[] = [
    ...commands,
    ...readerCommands,
    ...(currentFolder
      ? [
          { id: "cmd-rename-folder", group: "Action", label: `Rename “${currentFolder.name}”…`, run: () => renameFolderNamed(currentFolder) },
          { id: "cmd-delete-folder", group: "Action", label: `Delete “${currentFolder.name}”…`, run: () => deleteFolderNamed(currentFolder) },
        ]
      : []),
  ];

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const mod = event.metaKey || event.ctrlKey;
      const readerAction = reading && ws.overlay.kind === "none" && mod && !event.altKey ? readerKey(event) : null;
      if (mod && event.key.toLowerCase() === "k") {
        event.preventDefault();
        dispatch({ type: "overlay", overlay: { kind: ws.overlay.kind === "palette" ? "none" : "palette" } });
      } else if (mod && event.key === ",") {
        event.preventDefault();
        dispatch({ type: "overlay", overlay: { kind: "settings" } });
      } else if (event.metaKey && event.ctrlKey && event.key.toLowerCase() === "s") {
        event.preventDefault();
        toggleSidebar();
      } else if (mod && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "r") {
        event.preventDefault();
        if (scanPhase !== "running") void startScan();
      } else if (reading && readerAction) {
        event.preventDefault();
        const run = actionsRef.current;
        if (readerAction === "copy") run.copyMarkdown(reading.id);
        else if (readerAction === "save") run.saveFile(reading.id);
        else run.forget(reading.id, reading.title);
      } else if (
        (event.key === "/" && !mod && !event.altKey && !isTyping(event.target)) ||
        (mod && event.altKey && event.code === "KeyF")
      ) {
        event.preventDefault();
        queryRef.current?.focus();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [reading, scanPhase, startScan, toggleSidebar, ws.overlay.kind]);

  const scanning = ws.scan.phase === "running";
  const catalogState = catalog.state;
  const archiveEmpty =
    catalogState.status === "ready" &&
    scope.kind === "all" &&
    !searching &&
    browse.state.status === "ready" &&
    browse.state.items.length === 0 &&
    repositories.length === 0 &&
    agents.every((a) => a.session_count === 0);

  const plural = (n: number) => `${n} ${n === 1 ? "session" : "sessions"}`;
  const scopeCount = selectedRow?.count ?? null;
  // An agent row is a saved search with an exact count in the sidebar; any other
  // search counts the sessions it has loaded, and says "More than" while pages remain.
  const searchTitled = searching && (selected.kind !== "agent" || selectedRow === null);
  const searchState = search.state;
  const found = searchState.status === "ready" ? groupHits(searchState.items).length : null;
  const subtitle = scanning
    ? "Scanning…"
    : searchTitled
      ? searchPending || searchState.status !== "ready" || found === null
        ? ""
        : found === 0
          ? "No results"
          : searchState.cursor === null
            ? plural(found)
            : `More than ${plural(found)}`
      : catalogState.status !== "ready" || scopeCount === null
        ? ""
        : plural(scopeCount);
  const title = searchTitled ? `Searching “${currentRepo?.display_name ?? "All Sessions"}”` : scopeLabel;
  const setSort = (sort: SearchSort) => dispatch({ type: "sort", sort });
  const filterValues = useMemo<FilterValues>(
    () => ({
      "agent:": agents.map((a) => ({ value: a.id, count: a.session_count })),
      "repo:": repositories.map((r) => ({ value: r.display_name, count: r.session_count })),
    }),
    [agents, repositories],
  );

  return (
    <TooltipProvider>
      <div className={clsx("grid h-full", sidebar.visible ? "grid-cols-window" : "grid-cols-window-collapsed")}>
        {sidebar.visible && (
          <Sidebar
            sections={sections}
            selected={selected}
            onChoose={(source) => chooseSource(source, "stay")}
            onToggle={sidebar.toggle}
            folderActions={folderActions}
          />
        )}
        <div className="flex min-h-0 min-w-0 flex-col">
          <Toolbar
            insetForTrafficLights={!sidebar.visible}
            list={
              <>
                {!sidebar.visible && <SidebarToggle visible={false} onToggle={sidebar.toggle} />}
                {sidebar.visible ? (
                  <ScopeTitle title={title} subtitle={subtitle} />
                ) : (
                  <ScopeMenu
                    title={title}
                    subtitle={subtitle}
                    sort={searching ? { value: ws.sort, onChange: setSort } : null}
                    sections={sections}
                    selected={selected}
                    onChoose={chooseSource}
                    onNewFolder={newFolder}
                  />
                )}
              </>
            }
            reader={
              <>
                <ReaderToolbar
                  session={reading}
                  folders={folders}
                  actions={actions}
                  note={actionNote}
                  toolCalls={toolCalls}
                  onToolCalls={setToolCalls}
                />
                <div data-tauri-drag-region className="h-full min-w-0 flex-1" />
                <ScanControl scan={ws.scan} lastScanAt={lastScanAt} onRescan={() => void startScan()} />
                <Tooltip label="Command Palette (⌘K)">
                  <Button
                    variant="toolbar"
                    aria-label="Command Palette"
                    aria-keyshortcuts="Meta+K"
                    onClick={() => overlay({ kind: "palette" })}
                  >
                    <CommandIcon className={ICON.toolbar} aria-hidden="true" />
                  </Button>
                </Tooltip>
                <SearchField
                  value={ws.query}
                  onChange={(query) => dispatch({ type: "query", query })}
                  inputRef={queryRef}
                  onLeaveDown={focusList}
                  values={filterValues}
                  sort={ws.sort}
                  onSort={setSort}
                />
              </>
            }
          />
          <MessageBar flash={ws.flash} onDismiss={() => dispatch({ type: "flash", flash: null })} />
          <main className="grid min-h-0 flex-1 grid-cols-panes">
            {catalogState.status === "failed" ? (
              <div className="placeholder placeholder--page col-all" role="alert">
                <p>Lore could not open the archive.</p>
                <p className="quiet">{catalogState.error}</p>
                <p className="quiet">Your agent logs were not changed.</p>
                <button type="button" className="btn" onClick={() => void refreshAll()}>
                  Try again
                </button>
              </div>
            ) : archiveEmpty ? (
              <div className="col-all min-h-0 overflow-auto">
                <Onboarding
                  agents={agents}
                  scanning={scanning}
                  busyAgent={busyAgent}
                  onScan={() => void startScan()}
                  onAddRoot={(agent) => void addRoot(agent)}
                  onOpenSettings={() => overlay({ kind: "settings" })}
                />
              </div>
            ) : (
              <>
                <div className="flex min-h-0 min-w-0 border-r bg-bg-content select-none">
                  {searching ? (
                    <SearchPane
                      query={settledQuery}
                      terms={terms}
                      filters={queryFilters(settledQuery)}
                      pending={searchPending}
                      scopeBar={
                        <ScopeBar
                          scope={
                            currentRepo
                              ? { label: currentRepo.display_name, narrows: true }
                              : currentFolder
                                ? { label: currentFolder.name, narrows: false }
                                : null
                          }
                          onSearchAll={() => chooseSource(ALL, "stay")}
                          sort={ws.sort}
                          onSort={setSort}
                        />
                      }
                      state={search.state}
                      openId={ws.openId}
                      onOpen={openSession}
                      onMore={() => void search.loadMore()}
                      listRef={listRef}
                      onExitTop={focusQuery}
                      onSearchAll={currentRepo ? () => chooseSource(ALL) : null}
                      onClearFilters={() => dispatch({ type: "query", query: withoutFilters(ws.query) })}
                    />
                  ) : (
                    <BrowsePane
                      label={scopeLabel}
                      state={catalogState.status === "loading" ? { status: "loading" } : browse.state}
                      openId={ws.openId}
                      onOpen={openSession}
                      onMore={() => void browse.loadMore()}
                      onRetry={() => void refreshAll()}
                      listRef={listRef}
                      onExitTop={focusQuery}
                      emptyHint={
                        currentFolder
                          ? "This folder is empty. Open a session and choose File in folder."
                          : currentRepo
                            ? "No sessions are linked to this repository."
                            : "No sessions yet. Rescan to read your agent history."
                      }
                    />
                  )}
                </div>
                <div className="min-h-0 min-w-0 overflow-auto bg-bg-content select-text">
                  <Reader
                    state={open.state}
                    onRetry={open.retry}
                    terms={searching ? terms : []}
                    repositories={repositories}
                    toolCalls={toolCalls}
                    onBack={focusList}
                    readerRef={readerRef}
                  />
                </div>
              </>
            )}
          </main>
        </div>
      </div>

      {ws.overlay.kind === "palette" && (
        <CommandPalette commands={paletteCommands} searchArchive={searchArchive} onClose={closeOverlay} />
      )}
      {ws.overlay.kind === "settings" && (
        <SettingsDialog
          agents={agents}
          busyAgent={busyAgent}
          onAddRoot={(agent) => void addRoot(agent)}
          onRemoveRoot={(agent, path) => void removeRoot(agent, path)}
          onForgetEverything={forgetAll}
          onClose={closeOverlay}
        />
      )}
      {ws.overlay.kind === "confirm" && (
        <ConfirmDialog
          title={ws.overlay.title}
          body={ws.overlay.body}
          confirmLabel={ws.overlay.confirmLabel}
          onConfirm={ws.overlay.onConfirm}
          onClose={closeOverlay}
        />
      )}
      {ws.overlay.kind === "name" && (
        <NameDialog
          title={ws.overlay.title}
          label={ws.overlay.label}
          initial={ws.overlay.initial}
          confirmLabel={ws.overlay.confirmLabel}
          onSubmit={ws.overlay.onSubmit}
          onClose={closeOverlay}
        />
      )}
    </TooltipProvider>
  );
}
