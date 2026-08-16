import { useState } from "react";

import type { RepositorySummary } from "../ipc";
import { SESSION_DND_MIME } from "./SessionList";

export default function RepositoryList({
  repositories,
  selectedId,
  allSelected,
  onSelect,
  onUnfileSession,
}: {
  repositories: RepositorySummary[];
  selectedId: string | null;
  /**
   * True when the unfiltered "All sessions" view is active (no repo or folder).
   * Defaults to "no repo selected" for callers that have no folder dimension.
   */
  allSelected?: boolean;
  onSelect: (id: string | null) => void;
  /** Drop a thread here to remove it from its folder. */
  onUnfileSession?: (sessionId: string) => void;
}) {
  const isAll = allSelected ?? selectedId === null;
  const [unfileHover, setUnfileHover] = useState(false);
  const hasSession = (event: React.DragEvent) =>
    event.dataTransfer.types.includes(SESSION_DND_MIME);

  return (
    <nav className="repos" aria-label="Repositories">
      <button
        type="button"
        className={`nav-item${unfileHover ? " is-dragover" : ""}`}
        aria-pressed={isAll}
        onClick={() => onSelect(null)}
        onDragOver={(event) => {
          if (!onUnfileSession || !hasSession(event)) return;
          event.preventDefault();
          event.dataTransfer.dropEffect = "move";
          setUnfileHover(true);
        }}
        onDragLeave={() => setUnfileHover(false)}
        onDrop={(event) => {
          if (!onUnfileSession || !hasSession(event)) return;
          event.preventDefault();
          setUnfileHover(false);
          const sessionId = event.dataTransfer.getData(SESSION_DND_MIME);
          if (sessionId) onUnfileSession(sessionId);
        }}
      >
        <span className="dot dot--accent" aria-hidden />
        <span className="nav-item__name">All sessions</span>
      </button>

      <h2 className="nav-heading">Repositories</h2>
      {repositories.length === 0 ? (
        <p className="repos__empty">None resolved yet.</p>
      ) : (
        <ul className="nav-list">
          {repositories.map((repo) => (
            <li key={repo.id}>
              <button
                type="button"
                className="nav-item"
                aria-pressed={selectedId === repo.id}
                onClick={() => onSelect(repo.id)}
              >
                <span
                  className={`dot dot--${repo.identity_confidence}`}
                  aria-label={`${repo.identity_confidence} confidence`}
                  title={`${repo.identity_confidence} confidence`}
                />
                <span className="nav-item__name">{repo.display_name}</span>
                {repo.is_missing && <span className="chip chip--danger">missing</span>}
                <span className="nav-item__count">{repo.session_count}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </nav>
  );
}
