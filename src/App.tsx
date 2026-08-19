import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import CommandPalette, { type Command } from "./components/CommandPalette";
import ArchiveOnboarding from "./components/ArchiveOnboarding";
import FolderList from "./components/FolderList";
import RepositoryList from "./components/RepositoryList";
import SearchResults from "./components/SearchResults";
import SessionList from "./components/SessionList";
import SessionView from "./components/SessionView";
import SettingsPanel from "./components/SettingsPanel";
import { agentLabel } from "./format";
import {
  addAgentRoot,
  chooseAgentRootDirectory,
  createFolder,
  deleteFolder,
  exportSessionMarkdown,
  forgetEverything,
  forgetSession,
  getFilePatch,
  getGitSnapshot,
  getSession,
  getSetting,
  listDetectedAgents,
  listFolderSessionsPage,
  listFolders,
  listRepositories,
  listRepositorySessionsPage,
  listSessionsPage,
  onScanProgress,
  removeAgentRoot,
  renameFolder,
  rescan,
  saveSessionExport,
  searchPage,
  sessionSecretCount,
  setSessionFolder,
  setSetting,
  type DetectedAgent,
  type FolderSummary,
  type GitObservationDto,
  type RepositorySummary,
  type ScanProgress,
  type SearchHit,
  type SessionDetail,
  type SessionSummary,
} from "./ipc";

/** Sessions fetched per browse page. Initial navigation stays bounded. */
const SESSION_PAGE = 100;
/** Search results fetched per page; "Load more" appends another page. */
const SEARCH_PAGE = 50;
/** Coalesce rapid typing before crossing the Tauri/SQLite boundary. */
const SEARCH_DEBOUNCE_MS = 180;

/** The Lore mark: a node linked to two overlapping rings. */
function Mark() {
  return (
    <svg className="shell__mark" viewBox="0 0 140 96" fill="none" aria-hidden="true">
      <circle
        cx="90"
        cy="48"
        r="30"
        stroke="currentColor"
        strokeWidth="7"
        strokeLinecap="round"
        strokeDasharray="176.98 11.52"
        transform="rotate(191 90 48)"
      />
      <circle
        cx="64"
        cy="48"
        r="20"
        stroke="currentColor"
        strokeWidth="7"
        strokeLinecap="round"
        strokeDasharray="117.98 7.68"
        transform="rotate(191 64 48)"
      />
      <circle cx="28" cy="48" r="7" fill="currentColor" />
    </svg>
  );
}

function toggleTheme() {
  const root = document.documentElement;
  const next =
    root.dataset.theme === "dark"
      ? "light"
      : root.dataset.theme === "light"
        ? "dark"
        : matchMedia("(prefers-color-scheme: dark)").matches
          ? "light"
          : "dark";
  root.dataset.theme = next;
  // Persist so the choice survives restarts and archive clearing. Fire-and-forget:
  // a failed write must not break the toggle.
  void setSetting("theme", JSON.stringify(next)).catch(() => {});
}

/**
 * The M5 three-pane shell: repositories (left), the sessions list (middle), and
 * the session reader with its git rail (right). Under active development.
 */
export default function App() {
  const [archiveStatus, setArchiveStatus] = useState<"loading" | "ready" | "failed">(
    "loading",
  );
  const [agents, setAgents] = useState<DetectedAgent[]>([]);
  const [repositories, setRepositories] = useState<RepositorySummary[]>([]);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [sessionCursor, setSessionCursor] = useState<string | null>(null);
  const [loadingMoreSessionsRequest, setLoadingMoreSessionsRequest] = useState<number | null>(null);
  const loadingMoreSessions = loadingMoreSessionsRequest !== null;
  const [selectedRepo, setSelectedRepo] = useState<string | null>(null);
  const [folders, setFolders] = useState<FolderSummary[]>([]);
  const [selectedFolder, setSelectedFolder] = useState<string | null>(null);
  const [selectedSession, setSelectedSession] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [git, setGit] = useState<GitObservationDto[]>([]);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [query, setQuery] = useState("");
  // `null` is the pending state; an array (including empty) is settled. Keeping
  // those mutually exclusive avoids a separate flag drifting from the results.
  const [hits, setHits] = useState<SearchHit[] | null>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  // Always holds the latest query so an in-flight page can tell it has been
  // superseded by newer typing and drop its (stale) results.
  const queryRef = useRef("");
  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchResultsRef = useRef<HTMLUListElement>(null);
  const selectedRepoRef = useRef<string | null>(null);
  const selectedFolderRef = useRef<string | null>(null);
  const refreshRequestRef = useRef(0);
  const sessionsRequestRef = useRef(0);
  const sessionsPendingRequestRef = useRef<number | null>(null);
  const loadedSessionCountRef = useRef(0);
  const sessionRequestRef = useRef(0);
  const sessionPendingRequestRef = useRef<number | null>(null);
  const [secretCount, setSecretCount] = useState(0);
  const [notice, setNotice] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [rootBusy, setRootBusy] = useState<string | null>(null);

  const loadSessions = useCallback(
    async (repo: string | null, folder: string | null, showPending = false) => {
    const request = ++sessionsRequestRef.current;
    setLoadingMoreSessionsRequest(null);
    const limit = showPending
      ? SESSION_PAGE
      : Math.max(SESSION_PAGE, loadedSessionCountRef.current);
    if (showPending) {
      sessionsPendingRequestRef.current = request;
      loadedSessionCountRef.current = 0;
      setSessions([]);
      setSessionCursor(null);
    }
    try {
      const page = folder
        ? await listFolderSessionsPage(folder, limit, null)
        : repo
          ? await listRepositorySessionsPage(repo, limit, null)
          : await listSessionsPage(limit, null);
      if (request === sessionsRequestRef.current) {
        // The newest request owns the pane and also settles any pending marker
        // inherited from a manual load that it superseded.
        sessionsPendingRequestRef.current = null;
        loadedSessionCountRef.current = page.sessions.length;
        setSessions(page.sessions);
        setSessionCursor(page.next_cursor);
      }
    } catch (e) {
      if (request !== sessionsRequestRef.current) return;
      sessionsPendingRequestRef.current = null;
      loadedSessionCountRef.current = 0;
      setSessions([]);
      setSessionCursor(null);
      throw e;
    }
  }, []);

  const refresh = useCallback(async () => {
    const request = ++refreshRequestRef.current;
    const repo = selectedRepoRef.current;
    const folder = selectedFolderRef.current;
    try {
      const nextAgents = await listDetectedAgents();
      if (request !== refreshRequestRef.current) return;
      setAgents(nextAgents);
      const nextRepositories = await listRepositories();
      if (request !== refreshRequestRef.current) return;
      setRepositories(nextRepositories);
      const nextFolders = await listFolders();
      if (request !== refreshRequestRef.current) return;
      setFolders(nextFolders);
      // A manual repository/folder change owns its own load. Do not duplicate or
      // supersede it if the selection changed while this refresh was running.
      if (repo === selectedRepoRef.current && folder === selectedFolderRef.current)
        await loadSessions(repo, folder);
      if (request === refreshRequestRef.current) setArchiveStatus("ready");
    } catch (e) {
      if (request === refreshRequestRef.current) {
        setError(String(e));
        // A later background failure must not erase a previously successful
        // archive load, but a failed first load is never an "empty archive".
        setArchiveStatus((current) => (current === "ready" ? current : "failed"));
      }
    }
  }, [loadSessions]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let refreshTimer: ReturnType<typeof setTimeout> | null = null;
    const unlisten = onScanProgress((next) => {
      setProgress(next);
      if (next.done) setScanning(false);
      // Coalesce progress storms into one archive refresh. This makes results
      // appear as the background worker commits them without issuing a query
      // for every single ingested source.
      if (next.ingested > 0 || next.skipped > 0 || next.failed > 0) {
        if (refreshTimer !== null) clearTimeout(refreshTimer);
        refreshTimer = setTimeout(() => void refresh(), 300);
      }
    });
    return () => {
      if (refreshTimer !== null) clearTimeout(refreshTimer);
      void unlisten.then((off) => off());
    };
  }, [refresh]);

  // Restore a persisted theme choice on launch; absent one, the CSS default
  // (system preference) applies. Ignores an unset or malformed value.
  useEffect(() => {
    void getSetting("theme")
      .then((raw) => {
        if (!raw) return;
        const theme = JSON.parse(raw) as unknown;
        if (theme === "light" || theme === "dark") {
          document.documentElement.dataset.theme = theme;
        }
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
        return;
      }
      // `/` focuses search (Linear/Notion convention) unless the user is already
      // typing in a field, so `/` never hijacks a real keystroke.
      if (event.key === "/" && !event.metaKey && !event.ctrlKey && !event.altKey) {
        const target = event.target as HTMLElement | null;
        const editable =
          target?.isContentEditable ||
          target?.tagName === "INPUT" ||
          target?.tagName === "TEXTAREA" ||
          target?.tagName === "SELECT";
        if (!editable) {
          event.preventDefault();
          searchInputRef.current?.focus();
        }
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    if (query.trim() === "") {
      return;
    }

    const forQuery = query;
    let cancelled = false;
    const timer = setTimeout(() => {
      void searchPage(forQuery, SEARCH_PAGE)
        .then((page) => {
          if (cancelled || queryRef.current !== forQuery) return;
          setHits(page.hits);
          setCursor(page.next_cursor);
        })
        .catch((e: unknown) => {
          if (!cancelled && queryRef.current === forQuery) {
            setHits([]);
            setError(String(e));
          }
        });
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query]);

  const selectRepo = useCallback(async (repo: string | null) => {
    selectedRepoRef.current = repo;
    selectedFolderRef.current = null;
    setSelectedRepo(repo);
    setSelectedFolder(null);
    setError(null);
    try {
      await loadSessions(repo, null, true);
    } catch (e) {
      setError(String(e));
    }
  }, [loadSessions]);

  const selectFolder = useCallback(async (folderId: string) => {
    selectedFolderRef.current = folderId;
    selectedRepoRef.current = null;
    setSelectedFolder(folderId);
    setSelectedRepo(null);
    setError(null);
    try {
      await loadSessions(null, folderId, true);
    } catch (e) {
      setError(String(e));
    }
  }, [loadSessions]);

  const refreshFolders = useCallback(async () => {
    try {
      setFolders(await listFolders());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const handleCreateFolder = useCallback(
    async (name: string) => {
      setError(null);
      try {
        await createFolder(name);
        await refreshFolders();
      } catch (e) {
        setError(String(e));
      }
    },
    [refreshFolders],
  );

  const handleRenameFolder = useCallback(
    async (id: string, name: string) => {
      try {
        await renameFolder(id, name);
        await refreshFolders();
      } catch (e) {
        setError(String(e));
      }
    },
    [refreshFolders],
  );

  const handleDeleteFolder = useCallback(
    async (id: string) => {
      if (!window.confirm("Delete this folder? Its threads stay in Lore, just unfiled.")) {
        return;
      }
      try {
        await deleteFolder(id);
        // Leaving a now-deleted folder view falls back to All sessions.
        if (selectedFolderRef.current === id) {
          await selectRepo(null);
        }
        await refreshFolders();
      } catch (e) {
        setError(String(e));
      }
    },
    [refreshFolders, selectRepo],
  );

  // File a thread into a folder (or unfile it when `folderId` is null), then
  // refresh folder counts and the current pane so membership changes show at once.
  const fileSession = useCallback(
    async (sessionId: string, folderId: string | null) => {
      setError(null);
      try {
        await setSessionFolder(sessionId, folderId);
        await refreshFolders();
        await loadSessions(selectedRepoRef.current, selectedFolderRef.current);
        const target = folders.find((f) => f.id === folderId);
        setNotice(
          folderId
            ? `Filed thread in “${target?.name ?? "folder"}”.`
            : "Removed thread from its folder.",
        );
      } catch (e) {
        setError(String(e));
      }
    },
    [folders, loadSessions, refreshFolders],
  );

  function updateSearch(next: string) {
    // Batch the query and its visible state so snippets from the previous query
    // cannot paint under the new text while the debounce effect is pending.
    setQuery(next);
    queryRef.current = next;
    setHits(next.trim() === "" ? [] : null);
    setCursor(null);
    setError(null);
  }

  async function loadMore() {
    if (cursor === null || loadingMore) return;
    const forQuery = queryRef.current;
    setLoadingMore(true);
    try {
      const page = await searchPage(forQuery, SEARCH_PAGE, cursor);
      if (queryRef.current !== forQuery) return; // query changed mid-flight
      setHits((prev) => [...(prev ?? []), ...page.hits]);
      setCursor(page.next_cursor);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoadingMore(false);
    }
  }

  async function loadOlderSessions() {
    if (sessionCursor === null || loadingMoreSessions) return;
    const request = ++sessionsRequestRef.current;
    const repo = selectedRepoRef.current;
    const folder = selectedFolderRef.current;
    const forCursor = sessionCursor;
    setLoadingMoreSessionsRequest(request);
    setError(null);
    try {
      const page = folder
        ? await listFolderSessionsPage(folder, SESSION_PAGE, forCursor)
        : repo
          ? await listRepositorySessionsPage(repo, SESSION_PAGE, forCursor)
          : await listSessionsPage(SESSION_PAGE, forCursor);
      if (
        request !== sessionsRequestRef.current ||
        repo !== selectedRepoRef.current ||
        folder !== selectedFolderRef.current
      )
        return;
      // A cursor page should be disjoint, but deduping at the UI boundary keeps
      // a concurrent archive refresh from ever painting a duplicate.
      const seen = new Set(sessions.map((session) => session.id));
      const merged = [
        ...sessions,
        ...page.sessions.filter((session) => !seen.has(session.id)),
      ];
      loadedSessionCountRef.current = merged.length;
      setSessions(merged);
      setSessionCursor(page.next_cursor);
    } catch (e) {
      if (request === sessionsRequestRef.current) setError(String(e));
    } finally {
      setLoadingMoreSessionsRequest((current) => (current === request ? null : current));
    }
  }

  const openSession = useCallback(async (id: string) => {
    const request = ++sessionRequestRef.current;
    sessionPendingRequestRef.current = request;
    setSelectedSession(id);
    setError(null);
    setNotice(null);
    try {
      const [loaded, snapshot, secrets] = await Promise.all([
        getSession(id),
        getGitSnapshot(id),
        sessionSecretCount(id),
      ]);
      if (request !== sessionRequestRef.current) return;
      sessionPendingRequestRef.current = null;
      setDetail(loaded);
      setGit(snapshot);
      setSecretCount(secrets);
    } catch (e) {
      if (request !== sessionRequestRef.current) return;
      sessionPendingRequestRef.current = null;
      setSelectedSession(null);
      setDetail(null);
      setError(String(e));
    }
  }, []);

  async function handleExport() {
    if (!selectedSession) return;
    try {
      const markdown = await exportSessionMarkdown(selectedSession, false);
      if (markdown == null) return;
      if (!navigator.clipboard?.writeText) {
        setError("Clipboard is unavailable here. Use “Save file” instead.");
        return;
      }
      await navigator.clipboard.writeText(markdown);
      setNotice("Copied Markdown to the clipboard (flagged secrets redacted).");
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleSaveFile() {
    if (!selectedSession) return;
    try {
      const saved = await saveSessionExport(selectedSession, null, false);
      if (saved) {
        setNotice("Saved session export.");
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleForget() {
    if (!selectedSession) return;
    if (!window.confirm("Forget this session? This permanently removes it from Lore.")) {
      return;
    }
    try {
      sessionRequestRef.current += 1;
      sessionPendingRequestRef.current = null;
      const report = await forgetSession(selectedSession);
      setDetail(null);
      setSelectedSession(null);
      await refresh();
      const remaining =
        report.source_paths.length > 0
          ? ` The original agent log(s) remain: ${report.source_paths.join(", ")}.`
          : "";
      setNotice(`Session forgotten (${report.blobs_removed} blob(s) removed).${remaining}`);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleForgetEverything() {
    if (
      !window.confirm(
        "Forget everything? This permanently removes all sessions, repositories, and findings from Lore. Original agent logs are not touched.",
      )
    ) {
      return;
    }
    try {
      sessionRequestRef.current += 1;
      sessionPendingRequestRef.current = null;
      const report = await forgetEverything();
      setDetail(null);
      setSelectedSession(null);
      setSelectedRepo(null);
      selectedRepoRef.current = null;
      setSelectedFolder(null);
      selectedFolderRef.current = null;
      setGit([]);
      setQuery("");
      queryRef.current = "";
      setHits([]);
      setCursor(null);
      setSettingsOpen(false);
      await refresh();
      setNotice(`Archive cleared (${report.blobs_removed} blob(s) removed).`);
    } catch (e) {
      setError(String(e));
    }
  }

  const handleRescan = useCallback(async () => {
    setScanning(true);
    setError(null);
    try {
      await rescan();
      await refresh();
    } catch (e) {
      setError(String(e));
      setScanning(false);
    }
  }, [refresh]);

  const handleAddAgentRoot = useCallback(
    async (agentId: string, displayName: string) => {
      setRootBusy(agentId);
      setError(null);
      try {
        const selected = await chooseAgentRootDirectory(displayName);
        if (selected === null) return;
        await addAgentRoot(agentId, selected);
        setNotice(`${displayName} folder added. Lore is scanning it now.`);
        await refresh();
      } catch (e) {
        setError(String(e));
      } finally {
        setRootBusy(null);
      }
    },
    [refresh],
  );

  const handleRemoveAgentRoot = useCallback(
    async (agentId: string, path: string) => {
      setRootBusy(agentId);
      setError(null);
      try {
        await removeAgentRoot(agentId, path);
        setNotice("Custom folder removed. Already archived sessions are unchanged.");
        await refresh();
      } catch (e) {
        setError(String(e));
      } finally {
        setRootBusy(null);
      }
    },
    [refresh],
  );

  const searchPaletteArchive = useCallback(
    async (rawQuery: string): Promise<Command[]> => {
      const page = await searchPage(rawQuery, SEARCH_PAGE);
      const seen = new Set<string>();
      const commands: Command[] = [];
      for (const hit of page.hits) {
        if (seen.has(hit.session_id)) continue;
        seen.add(hit.session_id);
        commands.push({
          id: `cmd-sess-${hit.session_id}`,
          group: "Archive",
          label: hit.title ?? "(untitled session)",
          hint: `${agentLabel(hit.agent_id)} · archive match`,
          run: () => void openSession(hit.session_id),
        });
      }
      return commands;
    },
    [openSession],
  );

  const commands = useMemo<Command[]>(() => {
    const actions: Command[] = [
      { id: "cmd-rescan", group: "Action", label: "Rescan", run: () => void handleRescan() },
      { id: "cmd-all", group: "Action", label: "All sessions", run: () => void selectRepo(null) },
      {
        id: "cmd-new-folder",
        group: "Action",
        label: "New folder…",
        run: () => {
          const name = window.prompt("New folder name");
          if (name && name.trim()) void handleCreateFolder(name);
        },
      },
      {
        id: "cmd-agent-folders",
        group: "Action",
        label: "Manage agent folders",
        hint: "Settings",
        run: () => setSettingsOpen(true),
      },
    ];
    const folderCommands: Command[] = folders.map((folder) => ({
      id: `cmd-folder-${folder.id}`,
      group: "Folder",
      label: folder.name,
      hint: `${folder.session_count} threads`,
      run: () => void selectFolder(folder.id),
    }));
    const repoCommands: Command[] = repositories.map((repo) => ({
      id: `cmd-repo-${repo.id}`,
      group: "Repository",
      label: repo.display_name,
      hint: `${repo.session_count} sessions`,
      run: () => void selectRepo(repo.id),
    }));
    const sessionCommands: Command[] = sessions.map((session) => ({
      id: `cmd-sess-${session.id}`,
      group: "Session",
      label: session.title ?? "(untitled)",
      hint: agentLabel(session.agent_id),
      run: () => void openSession(session.id),
    }));
    return [...actions, ...folderCommands, ...repoCommands, ...sessionCommands];
  }, [
    folders,
    handleCreateFolder,
    handleRescan,
    openSession,
    repositories,
    selectFolder,
    selectRepo,
    sessions,
  ]);

  const archiveEmpty =
    archiveStatus === "ready" &&
    selectedRepo === null &&
    repositories.length === 0 &&
    sessions.length === 0 &&
    agents.every((agent) => agent.session_count === 0);

  // Scan progress: processed vs discovered, so the header strip can show a real
  // fraction and bar rather than bare counters. Discovered is the candidate
  // count; processed is how many of those candidates have been resolved.
  const scanProcessed = progress ? progress.ingested + progress.skipped + progress.failed : 0;
  const scanTotal = progress ? Math.max(progress.discovered, scanProcessed, 0) : 0;
  const scanPercent = scanTotal > 0 ? Math.min(100, Math.round((scanProcessed / scanTotal) * 100)) : 0;

  return (
    <div className="shell">
      <header className="shell__bar">
        <Mark />
        <h1>Lore</h1>
        <span className="shell__tagline">git memory for coding agents</span>
        <button
          type="button"
          className="omnibar"
          onClick={() => setPaletteOpen(true)}
          aria-label="Open command palette"
        >
          Jump to…
          <span className="omnibar__hint">
            <kbd>⌘</kbd>
            <kbd>K</kbd>
          </span>
        </button>
        <span className="shell__dev">preview build</span>
        <button
          type="button"
          className="icon-btn"
          onClick={toggleTheme}
          aria-label="Toggle theme"
          title="Toggle light/dark"
        >
          ◐
        </button>
        <button
          type="button"
          className="icon-btn"
          onClick={() => setSettingsOpen(true)}
          aria-label="Settings"
          title="Settings"
        >
          ⚙
        </button>
        <button
          type="button"
          className="btn--primary"
          onClick={handleRescan}
          disabled={scanning}
        >
          {scanning ? "Scanning…" : "Rescan"}
        </button>
      </header>

      {progress && (
        <div className="shell__progress" role="status">
          <div
            className="shell__progress-bar"
            role="progressbar"
            aria-label="Archive scan progress"
            aria-valuenow={scanPercent}
            aria-valuemin={0}
            aria-valuemax={100}
          >
            <div className="shell__progress-fill" style={{ width: `${scanPercent}%` }} />
          </div>
          <span className="shell__progress-text">
            {progress.done
              ? `Scan complete · ${progress.ingested} ingested · ${progress.skipped} skipped · ${progress.failed} failed`
              : `${scanProcessed}/${scanTotal} sessions · ${scanPercent}% · ${progress.failed} failed`}
          </span>
        </div>
      )}
      {error && (
        <div className="shell__error" role="alert">
          <span>{error}</span>
          <button
            type="button"
            className="shell__banner-dismiss"
            aria-label="Dismiss error"
            onClick={() => setError(null)}
          >
            ✕
          </button>
        </div>
      )}
      {notice && (
        <div className="shell__notice" role="status">
          <span>{notice}</span>
          <button
            type="button"
            className="shell__banner-dismiss"
            aria-label="Dismiss notice"
            onClick={() => setNotice(null)}
          >
            ✕
          </button>
        </div>
      )}

      <div className="shell__panes">
        <aside className="pane pane--repos">
          <RepositoryList
            repositories={repositories}
            selectedId={selectedRepo}
            allSelected={selectedRepo === null && selectedFolder === null}
            onSelect={selectRepo}
            onUnfileSession={(id) => void fileSession(id, null)}
          />
          <FolderList
            folders={folders}
            selectedId={selectedFolder}
            onSelect={(id) => void selectFolder(id)}
            onCreate={(name) => void handleCreateFolder(name)}
            onRename={(id, name) => void handleRenameFolder(id, name)}
            onDelete={(id) => void handleDeleteFolder(id)}
            onDropSession={(folderId, sessionId) => void fileSession(sessionId, folderId)}
          />
          {agents.length > 0 && (
            <p className="pane__agents">
              {agents.map((a) => `${a.display_name} (${a.session_count})`).join(" · ")}
            </p>
          )}
        </aside>

        <section className="pane pane--sessions" aria-label="Sessions">
          <input
            ref={searchInputRef}
            className="search-box"
            type="search"
            placeholder="Search… (e.g. retryBackoff agent:codex path:auth/)"
            aria-label="Search"
            value={query}
            onChange={(event) => updateSearch(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown" && hits !== null && hits.length > 0) {
                event.preventDefault();
                searchResultsRef.current?.focus();
              }
            }}
          />
          {query.trim() ? (
            <SearchResults
              ref={searchResultsRef}
              hits={hits ?? []}
              query={query}
              selectedId={selectedSession}
              onOpen={openSession}
              searching={hits === null}
              hasMore={cursor !== null}
              loadingMore={loadingMore}
              onLoadMore={() => void loadMore()}
              onExitUp={() => searchInputRef.current?.focus()}
            />
          ) : (
            archiveStatus === "loading" || sessionsPendingRequestRef.current !== null ? (
              <div className="sessions__empty" role="status">
                <p>Loading sessions…</p>
              </div>
            ) : archiveStatus === "failed" ? (
              <div className="sessions__empty" role="status">
                <p>Sessions unavailable.</p>
                <p className="empty">Fix the error above, then try Rescan.</p>
              </div>
            ) : (
              <SessionList
                sessions={sessions}
                selectedId={selectedSession}
                onOpen={openSession}
                hasMore={sessionCursor !== null}
                loadingMore={loadingMoreSessions}
                onLoadMore={() => void loadOlderSessions()}
              />
            )
          )}
        </section>

        <section className="pane pane--detail">
          {archiveStatus === "loading" ? (
            <section className="session session--empty" aria-label="session">
              <p role="status">Loading archive…</p>
            </section>
          ) : archiveStatus === "failed" ? (
            <section className="session session--empty" aria-label="session">
              <div role="status">
                <p>Archive unavailable.</p>
                <p className="empty">No local agent logs were changed. Retry with Rescan.</p>
              </div>
            </section>
          ) : archiveEmpty ? (
            <ArchiveOnboarding
              agents={agents}
              scanning={scanning}
              rootBusy={rootBusy}
              onScan={() => void handleRescan()}
              onAddAgentRoot={(agentId, displayName) =>
                void handleAddAgentRoot(agentId, displayName)
              }
              onOpenSettings={() => setSettingsOpen(true)}
            />
          ) : sessionPendingRequestRef.current !== null ? (
            <section className="session session--empty" aria-label="session">
              <p role="status">Loading session…</p>
            </section>
          ) : (
            <SessionView
              detail={detail}
              git={git}
              loadPatch={getFilePatch}
              secretCount={secretCount}
              onExport={handleExport}
              onSaveFile={handleSaveFile}
              onForget={handleForget}
            />
          )}
        </section>
      </div>

      {paletteOpen && (
        <CommandPalette
          items={commands}
          search={searchPaletteArchive}
          onClose={() => setPaletteOpen(false)}
        />
      )}
      <SettingsPanel
        open={settingsOpen}
        agents={agents}
        rootBusy={rootBusy}
        onAddAgentRoot={(agentId, displayName) =>
          void handleAddAgentRoot(agentId, displayName)
        }
        onRemoveAgentRoot={(agentId, path) => void handleRemoveAgentRoot(agentId, path)}
        onForgetEverything={handleForgetEverything}
        onClose={() => setSettingsOpen(false)}
      />
    </div>
  );
}
