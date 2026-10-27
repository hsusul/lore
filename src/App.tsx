import { clsx } from "clsx";
import {
  ArrowDownUp,
  Command as CommandIcon,
  Copy,
  Download,
  FileDiff,
  Folder,
  FolderGit2,
  FolderInput,
  FolderMinus,
  FolderPlus,
  GitBranch,
  Inbox,
  ListChevronsUpDown,
  ListFilter,
  MessageSquare,
  Monitor,
  Moon,
  PanelLeft,
  Pencil,
  RefreshCw,
  Rows2,
  Rows3,
  Rows4,
  Search,
  Settings,
  Sun,
  Trash2,
  type LucideIcon,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";

import CommandPalette, { type Command } from "./components/CommandPalette";
import { BrowsePane, groupHits, ScopeBar, SearchPane, type SessionMenu } from "./components/ListPane";
import type { OpenVia } from "./components/ListBox";
import Onboarding from "./components/Onboarding";
import Reader, { READER_TABS, type ReaderTab } from "./components/Reader";
import ReaderToolbar, { SessionMenuItems, type ActionNote, type ReaderActions } from "./components/ReaderToolbar";
import ScanControl from "./components/ScanControl";
import SearchField, { SORTS, type FilterValues } from "./components/SearchField";
import SettingsDialog from "./components/SettingsDialog";
import { Alert, NameSheet } from "./components/Sheets";
import Sidebar, { SidebarToggle } from "./components/Sidebar";
import { sourceRow, sourceSections } from "./components/sources";
import Toolbar, { ScopeMenu, ScopeTitle } from "./components/Toolbar";
import { Button, ICON } from "./components/ui/button";
import { Tooltip, TooltipProvider } from "./components/ui/tooltip";
import { formatMailDate } from "./format";
import { isRunnable, queryFilters, queryTerms, withoutFilters } from "./hooks/query";
import { useCatalog, type Catalog } from "./hooks/useCatalog";
import { useDebounced } from "./hooks/useDebounced";
import { useLastScan } from "./hooks/useLastScan";
import { useNote } from "./hooks/useNote";
import { useOpenSession } from "./hooks/useOpenSession";
import { usePagedList, type FetchPage } from "./hooks/usePagedList";
import { PREFERENCES, usePreferences, type PreferenceName } from "./hooks/usePreferences";
import { useSidebar } from "./hooks/useSidebar";
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

/** Icons for the palette's view choices, by preference and value. */
const CHOICE_ICONS: Record<string, LucideIcon> = {
  "theme:system": Monitor,
  "theme:light": Sun,
  "theme:dark": Moon,
  "density:compact": Rows4,
  "density:comfortable": Rows3,
  "density:spacious": Rows2,
  "toolCalls:collapsed": ListChevronsUpDown,
  "toolCalls:expanded": ListChevronsUpDown,
};

/** How the palette words a view choice: "Use Dark Appearance", "Show Tool Calls Expanded". */
function choiceLabel(name: PreferenceName, label: string): string {
  if (name === "theme") return `Use ${label} Appearance`;
  if (name === "density") return `Use ${label} Density`;
  const { title } = PREFERENCES[name];
  return `${title} ${label}`;
}

const TAB_ICONS: Record<ReaderTab, LucideIcon> = { conversation: MessageSquare, files: FileDiff, git: GitBranch };

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
  const sidebar = useSidebar();
  const lastScanAt = useLastScan(ws.scan);
  const [prefs, setPreference] = usePreferences();
  const toolCalls = prefs.toolCalls;
  const [actionNote, showNote] = useNote<ActionNote>();
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [filterReference, setFilterReference] = useState(false);
  // The tab belongs to the open session: opening another starts it on Conversation.
  const [tabFor, setTabFor] = useState<{ id: string | null; tab: ReaderTab }>({ id: null, tab: "conversation" });
  const readerTab = tabFor.id === ws.openId ? tabFor.tab : "conversation";
  const openId = ws.openId;
  const setReaderTab = useCallback((tab: ReaderTab) => setTabFor({ id: openId, tab }), [openId]);

  const queryRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const readerRef = useRef<HTMLElement>(null);
  const focusReaderOnLoad = useRef(false);

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

  // A failed reload keeps the rows on screen; the Rescan control's warning says so and retries (spec 9.9).
  const refreshRef = useRef<() => Promise<void>>(async () => {});
  const refreshAll = useCallback(async () => {
    try {
      await Promise.all([catalog.refresh(), browse.refresh()]);
      setRefreshError(null);
    } catch (e) {
      setRefreshError(String(e));
    }
  }, [browse, catalog]);
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

  /** Ask for a folder for `agent` and read it; the added path, or null when the user cancels. */
  const addRoot = useCallback(
    async (agent: DetectedAgent) => {
      const path = await chooseAgentRootDirectory(agent.display_name);
      if (path === null) return null;
      await addAgentRoot(agent.id, path);
      await refreshAll();
      return path;
    },
    [refreshAll],
  );

  const removeRoot = useCallback(
    async (agent: DetectedAgent, path: string) => {
      await removeAgentRoot(agent.id, path);
      await refreshAll();
    },
    [refreshAll],
  );

  const forgetAll = useCallback(async () => {
    const report = await forgetEverything();
    dispatch({ type: "forgot-everything" });
    await refreshAll();
    return report;
  }, [refreshAll]);

  // Each of these confirms by what it changes: the new folder appears, the renamed one
  // reads its new name, the deleted one goes away. A failure stays in the sheet that asked.
  const newFolder = useCallback(
    () =>
      overlay({
        kind: "name",
        sheet: {
          title: "New Folder",
          label: "Name",
          initial: "",
          action: "Create",
          onSubmit: async (name) => {
            await createFolder(name);
            await refreshAll();
          },
        },
      }),
    [overlay, refreshAll],
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
      sheet: {
        title: "Rename Folder",
        label: "Name",
        initial: folder.name,
        action: "Rename",
        onSubmit: async (name) => {
          await renameFolder(folder.id, name);
          await refreshAll();
        },
      },
    });

  const deleteFolderNamed = (folder: FolderSummary) =>
    overlay({
      kind: "alert",
      alert: {
        title: `Delete “${folder.name}”?`,
        body: "The folder goes away. Its sessions stay in Lore, just not filed.",
        action: "Delete Folder",
        onConfirm: async () => {
          await deleteFolder(folder.id);
          if (scope.kind === "folder" && scope.id === folder.id) dispatch({ type: "source", source: ALL });
          await refreshAll();
        },
      },
    });
  const folderActions = { onNewFolder: newFolder, onRenameFolder: renameFolderNamed, onDeleteFolder: deleteFolderNamed };

  // Reader actions confirm and fail on their own controls (spec 9.12): Copy, Save and File
  // say so in their tooltips, the system save panel answers Save, Forget removes the row.
  const actions: ReaderActions = {
    copyMarkdown: (id) =>
      void (async () => {
        try {
          const markdown = await exportSessionMarkdown(id, false);
          if (markdown == null) return;
          if (!navigator.clipboard?.writeText) {
            showNote({ control: "copy", text: "The clipboard is not available here. Use Save as File instead." });
            return;
          }
          await navigator.clipboard.writeText(markdown);
          showNote({ control: "copy", text: "Copied" });
        } catch (e) {
          showNote({ control: "copy", text: `Could not copy: ${String(e)}` });
        }
      })(),
    saveFile: (id) =>
      void saveSessionExport(id, null, false).catch((e: unknown) => showNote({ control: "save", text: `Could not save: ${String(e)}` })),
    file: (id, folderId) =>
      void setSessionFolder(id, folderId).then(
        async () => {
          await refreshAll();
          const name = folders.find((f) => f.id === folderId)?.name;
          showNote({ control: "file", text: name ? `Filed in “${name}”` : "Removed from Folder" });
        },
        (e: unknown) => showNote({ control: "file", text: `Could not file: ${String(e)}` }),
      ),
    forget: (id, title) =>
      overlay({
        kind: "alert",
        alert: {
          title: `Forget “${title}”?`,
          body: "Lore removes this session and its search entries from the archive. The original agent log on this Mac is not changed.",
          action: "Forget Session",
          onConfirm: async () => {
            await forgetSession(id);
            dispatch({ type: "forgot", id });
            await refreshAll();
          },
        },
      }),
  };
  const actionsRef = useRef(actions);
  actionsRef.current = actions;
  const sessionMenu: SessionMenu = (session) => <SessionMenuItems session={session} folders={folders} actions={actions} />;

  const searchArchive = useCallback(
    async (q: string): Promise<Command[]> => {
      const page = await searchPage(q, SEARCH_PAGE);
      const seen = new Set<string>();
      return page.hits
        .filter((hit) => !seen.has(hit.session_id) && !!seen.add(hit.session_id))
        .map((hit) => ({
          id: `cmd-session-${hit.session_id}`,
          group: "Sessions",
          label: hit.title ?? "Untitled session",
          icon: MessageSquare,
          detail: formatMailDate(hit.started_at),
          run: () => openSession(hit.session_id),
        }));
    },
    [openSession],
  );

  const setSort = useCallback((sort: SearchSort) => dispatch({ type: "sort", sort }), []);
  const toggleSidebar = sidebar.toggle;
  const scanPhase = ws.scan.phase;
  const sessions = browse.state.status === "ready" ? browse.state.items : [];

  // Every action, its shortcut, and every view choice (spec 9.8, 11). Reader actions and
  // tabs appear while a session is open; folder actions while a folder is the scope.
  const paletteCommands: Command[] = [
    ...(reading
      ? [
          { id: "cmd-copy", label: "Copy as Markdown", icon: Copy, shortcut: "⇧⌘C", run: () => actions.copyMarkdown(reading.id) },
          { id: "cmd-save", label: "Save as File…", icon: Download, shortcut: "⌘S", run: () => actions.saveFile(reading.id) },
          ...folders.map((f) => ({
            id: `cmd-file-${f.id}`,
            label: `File in “${f.name}”`,
            icon: FolderInput,
            run: () => actions.file(reading.id, f.id),
          })),
          { id: "cmd-unfile", label: "Remove from Folder", icon: FolderMinus, run: () => actions.file(reading.id, null) },
          { id: "cmd-forget", label: "Forget Session…", icon: Trash2, shortcut: "⌘⌫", run: () => actions.forget(reading.id, reading.title) },
        ]
      : []),
    { id: "cmd-rescan", label: "Rescan", icon: RefreshCw, shortcut: "⌘R", run: () => void startScan() },
    { id: "cmd-search", label: "Search Sessions", icon: Search, shortcut: "⌥⌘F", run: () => requestAnimationFrame(focusQuery) },
    { id: "cmd-filters", label: "Filter Reference", icon: ListFilter, shortcut: "⌘/", run: () => setFilterReference(true) },
    { id: "cmd-new-folder", label: "New Folder…", icon: FolderPlus, run: newFolder },
    ...(currentFolder
      ? [
          { id: "cmd-rename-folder", label: `Rename “${currentFolder.name}”…`, icon: Pencil, run: () => renameFolderNamed(currentFolder) },
          { id: "cmd-delete-folder", label: `Delete “${currentFolder.name}”…`, icon: Trash2, run: () => deleteFolderNamed(currentFolder) },
        ]
      : []),
    { id: "cmd-settings", label: "Settings…", icon: Settings, shortcut: "⌘,", run: () => overlay({ kind: "settings" }) },
  ].map((command) => ({ ...command, group: "Actions" as const }));

  const viewCommands: Command[] = [
    ...(reading
      ? READER_TABS.map((t) => ({
          id: `cmd-tab-${t.id}`,
          label: `Show ${t.label}`,
          icon: TAB_ICONS[t.id],
          shortcut: `⌘${t.key}`,
          run: () => setReaderTab(t.id),
        }))
      : []),
    { id: "cmd-all", label: "Show All Sessions", icon: Inbox, run: () => chooseSource(ALL) },
    { id: "cmd-sidebar", label: sidebar.visible ? "Hide Sidebar" : "Show Sidebar", icon: PanelLeft, shortcut: "⌃⌘S", run: toggleSidebar },
    ...SORTS.filter((s) => s.value !== ws.sort).map((s) => ({
      id: `cmd-sort-${s.value}`,
      label: `Sort by ${s.label}`,
      icon: ArrowDownUp,
      run: () => setSort(s.value),
    })),
    ...(Object.keys(PREFERENCES) as PreferenceName[]).flatMap((name) =>
      PREFERENCES[name].choices
        .filter((choice) => choice.value !== prefs[name])
        .map((choice) => ({
          id: `cmd-${name}-${choice.value}`,
          label: choiceLabel(name, choice.label),
          icon: CHOICE_ICONS[`${name}:${choice.value}`],
          run: () => setPreference(name, choice.value),
        })),
    ),
  ].map((command) => ({ ...command, group: "View" as const }));

  const navigationCommands: Command[] = [
    ...sessions.map((s) => ({
      id: `cmd-session-${s.id}`,
      group: "Sessions" as const,
      label: s.title ?? "Untitled session",
      icon: MessageSquare,
      detail: formatMailDate(s.started_at),
      run: () => openSession(s.id),
    })),
    ...repositories.map((r) => ({
      id: `cmd-repo-${r.id}`,
      group: "Repositories" as const,
      label: r.display_name,
      icon: FolderGit2,
      detail: `${r.session_count} ${r.session_count === 1 ? "session" : "sessions"}`,
      run: () => chooseSource({ kind: "repository", id: r.id }),
    })),
    ...folders.map((f) => ({
      id: `cmd-folder-${f.id}`,
      group: "Folders" as const,
      label: f.name,
      icon: Folder,
      detail: `${f.session_count} ${f.session_count === 1 ? "session" : "sessions"}`,
      run: () => chooseSource({ kind: "folder", id: f.id }),
    })),
  ];

  // Shortcuts act only on the window, never under a sheet, except ⌘K, which also closes the palette.
  const overlayKind = ws.overlay.kind;
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const mod = event.metaKey || event.ctrlKey;
      if (mod && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "k") {
        event.preventDefault();
        dispatch({ type: "overlay", overlay: { kind: overlayKind === "palette" ? "none" : "palette" } });
        return;
      }
      if (overlayKind !== "none") return;
      const readerAction = reading && mod && !event.altKey ? readerKey(event) : null;
      const tab = mod && !event.shiftKey && !event.altKey ? READER_TABS.find((t) => t.key === event.key) : undefined;
      if (mod && event.key === ",") {
        event.preventDefault();
        dispatch({ type: "overlay", overlay: { kind: "settings" } });
      } else if (event.metaKey && event.ctrlKey && event.key.toLowerCase() === "s") {
        event.preventDefault();
        toggleSidebar();
      } else if (mod && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "r") {
        event.preventDefault();
        if (scanPhase !== "running") void startScan();
      } else if (mod && event.key === "/") {
        event.preventDefault();
        setFilterReference((shown) => !shown);
      } else if (reading && tab) {
        event.preventDefault();
        setReaderTab(tab.id);
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
  }, [overlayKind, reading, scanPhase, setReaderTab, startScan, toggleSidebar]);

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
            sections={archiveEmpty ? [] : sections}
            selected={selected}
            onChoose={(source) => chooseSource(source, "stay")}
            onToggle={sidebar.toggle}
            folderActions={archiveEmpty ? null : folderActions}
          />
        )}
        <div className="flex min-h-0 min-w-0 flex-col">
          <Toolbar
            insetForTrafficLights={!sidebar.visible}
            split={!archiveEmpty}
            list={
              <>
                {!sidebar.visible && <SidebarToggle visible={false} onToggle={sidebar.toggle} />}
                {archiveEmpty ? null : sidebar.visible ? (
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
                {!archiveEmpty && (
                  <ReaderToolbar
                    session={reading}
                    folders={folders}
                    actions={actions}
                    note={actionNote}
                    toolCalls={toolCalls}
                    onToolCalls={(mode) => setPreference("toolCalls", mode)}
                  />
                )}
                <div data-tauri-drag-region className="h-full min-w-0 flex-1" />
                <ScanControl
                  scan={ws.scan}
                  lastScanAt={lastScanAt}
                  onRescan={() => void startScan()}
                  refreshError={refreshError}
                  onRefresh={() => void refreshAll()}
                />
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
                {!archiveEmpty && (
                  <SearchField
                    value={ws.query}
                    onChange={(query) => dispatch({ type: "query", query })}
                    inputRef={queryRef}
                    onLeaveDown={focusList}
                    values={filterValues}
                    sort={ws.sort}
                    onSort={setSort}
                    reference={filterReference}
                    onReference={setFilterReference}
                  />
                )}
              </>
            }
          />
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
              <div className="col-all flex min-h-0 overflow-auto bg-bg-content">
                <Onboarding agents={agents} scanning={scanning} onScan={() => void startScan()} onAddRoot={addRoot} />
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
                      sessionMenu={sessionMenu}
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
                      sessionMenu={sessionMenu}
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
                    tab={readerTab}
                    onTab={setReaderTab}
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
        <CommandPalette commands={[...paletteCommands, ...viewCommands, ...navigationCommands]} searchArchive={searchArchive} onClose={closeOverlay} />
      )}
      {ws.overlay.kind === "settings" && (
        <SettingsDialog
          prefs={prefs}
          onPreference={setPreference}
          sort={ws.sort}
          onSort={setSort}
          agents={agents}
          onAddRoot={addRoot}
          onRemoveRoot={removeRoot}
          onForgetEverything={forgetAll}
          onClose={closeOverlay}
        />
      )}
      {ws.overlay.kind === "alert" && <Alert key={ws.overlay.alert.title} alert={ws.overlay.alert} onClose={closeOverlay} />}
      {ws.overlay.kind === "name" && <NameSheet key={ws.overlay.sheet.title} sheet={ws.overlay.sheet} onClose={closeOverlay} />}
    </TooltipProvider>
  );
}
