import { useCallback, useEffect, useMemo, useState } from "react";

import CommandPalette, { type Command } from "./components/CommandPalette";
import RepositoryList from "./components/RepositoryList";
import SearchResults from "./components/SearchResults";
import SessionList from "./components/SessionList";
import SessionView from "./components/SessionView";
import SettingsPanel from "./components/SettingsPanel";
import { agentLabel } from "./format";
import {
  exportSessionMarkdown,
  forgetSession,
  getFilePatch,
  getGitSnapshot,
  getSession,
  listDetectedAgents,
  listRepositories,
  listRepositorySessions,
  listSessions,
  onScanProgress,
  forgetEverything,
  rescan,
  search,
  sessionSecretCount,
  type DetectedAgent,
  type GitObservationDto,
  type RepositorySummary,
  type ScanProgress,
  type SearchHit,
  type SessionDetail,
  type SessionSummary,
} from "./ipc";

const SESSION_LIMIT = 500;

/** A small commit-graph mark. */
function Mark() {
  return (
    <svg className="shell__mark" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <circle cx="4" cy="4" r="2" fill="currentColor" />
      <circle cx="4" cy="12" r="2" fill="currentColor" />
      <circle cx="12" cy="8" r="2" fill="currentColor" />
      <path
        d="M4 6v4M5.7 5 10.3 7.2M5.7 11 10.3 8.8"
        stroke="currentColor"
        strokeWidth="1.3"
      />
    </svg>
  );
}

/**
 * The M5 three-pane shell: repositories (left), the sessions list (middle), and
 * the session reader with its git rail (right). Under active development.
 */
export default function App() {
  const [agents, setAgents] = useState<DetectedAgent[]>([]);
  const [repositories, setRepositories] = useState<RepositorySummary[]>([]);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [selectedRepo, setSelectedRepo] = useState<string | null>(null);
  const [selectedSession, setSelectedSession] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [git, setGit] = useState<GitObservationDto[]>([]);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [secretCount, setSecretCount] = useState(0);
  const [notice, setNotice] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);

  const loadSessions = useCallback(async (repo: string | null) => {
    const rows = repo
      ? await listRepositorySessions(repo, SESSION_LIMIT)
      : await listSessions(SESSION_LIMIT);
    setSessions(rows);
  }, []);

  const refresh = useCallback(async () => {
    try {
      setAgents(await listDetectedAgents());
      setRepositories(await listRepositories());
      await loadSessions(selectedRepo);
    } catch (e) {
      setError(String(e));
    }
  }, [loadSessions, selectedRepo]);

  useEffect(() => {
    void refresh();
    const unlisten = onScanProgress(setProgress);
    return () => void unlisten.then((off) => off());
    // Run once on mount; refresh is re-invoked explicitly elsewhere.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  async function selectRepo(repo: string | null) {
    setSelectedRepo(repo);
    setError(null);
    try {
      await loadSessions(repo);
    } catch (e) {
      setError(String(e));
    }
  }

  async function runSearch(next: string) {
    setQuery(next);
    if (next.trim() === "") {
      setHits([]);
      return;
    }
    try {
      setHits(await search(next, 50));
    } catch (e) {
      setError(String(e));
    }
  }

  async function openSession(id: string) {
    setSelectedSession(id);
    setError(null);
    setNotice(null);
    try {
      const [loaded, snapshot, secrets] = await Promise.all([
        getSession(id),
        getGitSnapshot(id),
        sessionSecretCount(id),
      ]);
      setDetail(loaded);
      setGit(snapshot);
      setSecretCount(secrets);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleExport() {
    if (!selectedSession) return;
    try {
      const markdown = await exportSessionMarkdown(selectedSession, false);
      if (markdown == null) return;
      await navigator.clipboard?.writeText(markdown);
      setNotice("Exported Markdown to the clipboard (flagged secrets redacted).");
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
      const report = await forgetEverything();
      setDetail(null);
      setSelectedSession(null);
      setSettingsOpen(false);
      await refresh();
      setNotice(`Archive cleared (${report.blobs_removed} blob(s) removed).`);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleRescan() {
    setScanning(true);
    setError(null);
    try {
      await rescan();
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setScanning(false);
    }
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
  }

  const commands = useMemo<Command[]>(() => {
    const actions: Command[] = [
      { id: "cmd-rescan", group: "Action", label: "Rescan", run: () => void handleRescan() },
      { id: "cmd-all", group: "Action", label: "All sessions", run: () => void selectRepo(null) },
    ];
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
    return [...actions, ...repoCommands, ...sessionCommands];
    // Handlers are stable across renders; rebuild only when the data changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repositories, sessions]);

  return (
    <div className="shell">
      <header className="shell__bar">
        <Mark />
        <h1>Lore</h1>
        <span className="shell__tagline">git memory for coding agents</span>
        <button
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
          className="icon-btn"
          onClick={toggleTheme}
          aria-label="Toggle theme"
          title="Toggle light/dark"
        >
          ◐
        </button>
        <button
          className="icon-btn"
          onClick={() => setSettingsOpen(true)}
          aria-label="Settings"
          title="Settings"
        >
          ⚙
        </button>
        <button className="btn--primary" onClick={handleRescan} disabled={scanning}>
          {scanning ? "Scanning…" : "Rescan"}
        </button>
      </header>

      {progress && (
        <p className="shell__progress" role="status">
          Discovered {progress.discovered} · ingested {progress.ingested} · skipped{" "}
          {progress.skipped} · failed {progress.failed}
          {progress.done ? " · done" : "…"}
        </p>
      )}
      {error && (
        <p className="shell__error" role="alert">
          {error}
        </p>
      )}
      {notice && (
        <p className="shell__notice" role="status">
          {notice}
        </p>
      )}

      <div className="shell__panes">
        <aside className="pane pane--repos">
          <RepositoryList
            repositories={repositories}
            selectedId={selectedRepo}
            onSelect={selectRepo}
          />
          {agents.length > 0 && (
            <p className="pane__agents">
              {agents.map((a) => `${a.display_name} (${a.session_count})`).join(" · ")}
            </p>
          )}
        </aside>

        <section className="pane pane--sessions" aria-label="sessions">
          <input
            className="search-box"
            type="search"
            placeholder="Search… (e.g. retryBackoff agent:codex path:auth/)"
            aria-label="search"
            value={query}
            onChange={(event) => void runSearch(event.target.value)}
          />
          {query.trim() ? (
            <SearchResults
              hits={hits}
              query={query}
              selectedId={selectedSession}
              onOpen={openSession}
            />
          ) : (
            <SessionList
              sessions={sessions}
              selectedId={selectedSession}
              onOpen={openSession}
            />
          )}
        </section>

        <section className="pane pane--detail">
          <SessionView
            detail={detail}
            git={git}
            loadPatch={getFilePatch}
            secretCount={secretCount}
            onExport={handleExport}
            onForget={handleForget}
          />
        </section>
      </div>

      <CommandPalette
        items={commands}
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
      />
      <SettingsPanel
        open={settingsOpen}
        agents={agents}
        onForgetEverything={handleForgetEverything}
        onClose={() => setSettingsOpen(false)}
      />
    </div>
  );
}
