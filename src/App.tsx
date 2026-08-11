import { useCallback, useEffect, useState } from "react";

import RepositoryList from "./components/RepositoryList";
import SessionList from "./components/SessionList";
import SessionView from "./components/SessionView";
import {
  getFilePatch,
  getGitSnapshot,
  getSession,
  listDetectedAgents,
  listRepositories,
  listRepositorySessions,
  listSessions,
  onScanProgress,
  rescan,
  type DetectedAgent,
  type GitObservationDto,
  type RepositorySummary,
  type ScanProgress,
  type SessionDetail,
  type SessionSummary,
} from "./ipc";

const SESSION_LIMIT = 500;

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

  async function selectRepo(repo: string | null) {
    setSelectedRepo(repo);
    setError(null);
    try {
      await loadSessions(repo);
    } catch (e) {
      setError(String(e));
    }
  }

  async function openSession(id: string) {
    setSelectedSession(id);
    setError(null);
    try {
      const [loaded, snapshot] = await Promise.all([getSession(id), getGitSnapshot(id)]);
      setDetail(loaded);
      setGit(snapshot);
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

  return (
    <div className="shell">
      <header className="shell__bar">
        <h1>Lore</h1>
        <span className="shell__tagline">git memory for coding agents</span>
        <span className="shell__dev">under active development — not a release</span>
        <button onClick={handleRescan} disabled={scanning}>
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
          <SessionList
            sessions={sessions}
            selectedId={selectedSession}
            onOpen={openSession}
          />
        </section>

        <section className="pane pane--detail">
          <SessionView detail={detail} git={git} loadPatch={getFilePatch} />
        </section>
      </div>
    </div>
  );
}
