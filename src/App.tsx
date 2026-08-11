import { useEffect, useState } from "react";

import {
  listDetectedAgents,
  listSessions,
  onScanProgress,
  rescan,
  type DetectedAgent,
  type ScanProgress,
  type SessionSummary,
} from "./ipc";

/**
 * The M0 shell: a minimal, honest window that can trigger a rescan and list
 * detected agents and recent sessions. Under active development — not a release.
 */
export default function App() {
  const [agents, setAgents] = useState<DetectedAgent[]>([]);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    try {
      setAgents(await listDetectedAgents());
      setSessions(await listSessions(200));
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    void refresh();
    const unlisten = onScanProgress(setProgress);
    return () => {
      void unlisten.then((off) => off());
    };
  }, []);

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
    <main className="app">
      <header className="app__header">
        <h1>Lore</h1>
        <p className="app__tagline">Your coding agents forget. Lore doesn&apos;t.</p>
        <button onClick={handleRescan} disabled={scanning}>
          {scanning ? "Scanning…" : "Rescan"}
        </button>
      </header>

      <p className="app__status" role="status">
        Under active development — not a release build.
      </p>

      {error && <p className="app__error" role="alert">{error}</p>}

      {progress && (
        <p className="app__progress">
          Discovered {progress.discovered} · ingested {progress.ingested} · skipped{" "}
          {progress.skipped} · failed {progress.failed}
          {progress.done ? " · done" : "…"}
        </p>
      )}

      <section aria-labelledby="agents-heading">
        <h2 id="agents-heading">Agents</h2>
        {agents.length === 0 ? (
          <p>No agents ingested yet.</p>
        ) : (
          <ul>
            {agents.map((agent) => (
              <li key={agent.id}>
                {agent.display_name} — {agent.session_count} sessions
              </li>
            ))}
          </ul>
        )}
      </section>

      <section aria-labelledby="sessions-heading">
        <h2 id="sessions-heading">Recent sessions</h2>
        {sessions.length === 0 ? (
          <p>No sessions yet. Run a rescan to ingest your agent history.</p>
        ) : (
          <ul>
            {sessions.map((session) => (
              <li key={session.id}>
                <span>{session.title ?? "(untitled)"}</span> · {session.agent_id} ·{" "}
                {session.message_count} messages
              </li>
            ))}
          </ul>
        )}
      </section>
    </main>
  );
}
