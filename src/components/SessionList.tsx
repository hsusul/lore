import { useEffect, useState } from "react";

import type { SessionSummary } from "../ipc";

/**
 * A keyboard-navigable session list (listbox pattern): Arrow Up/Down moves the
 * active row, Enter opens it, click selects and opens.
 */
export default function SessionList({
  sessions,
  selectedId,
  onOpen,
}: {
  sessions: SessionSummary[];
  selectedId: string | null;
  onOpen: (id: string) => void;
}) {
  const [active, setActive] = useState(0);

  // Keep the active index in range as the list changes.
  useEffect(() => {
    setActive((current) => Math.min(current, Math.max(sessions.length - 1, 0)));
  }, [sessions.length]);

  function onKeyDown(event: React.KeyboardEvent<HTMLUListElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((current) => Math.min(current + 1, sessions.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((current) => Math.max(current - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const session = sessions[active];
      if (session) onOpen(session.id);
    }
  }

  if (sessions.length === 0) {
    return <p className="sessions__empty">No sessions. Run a rescan to ingest history.</p>;
  }

  return (
    <ul
      className="sessions"
      role="listbox"
      aria-label="sessions"
      tabIndex={0}
      aria-activedescendant={sessions[active]?.id}
      onKeyDown={onKeyDown}
    >
      {sessions.map((session, index) => (
        <li
          key={session.id}
          id={session.id}
          role="option"
          aria-selected={session.id === selectedId}
          className={`sessions__item${index === active ? " is-active" : ""}`}
          onClick={() => {
            setActive(index);
            onOpen(session.id);
          }}
        >
          <span className="sessions__title">{session.title ?? "(untitled)"}</span>
          <span className="sessions__meta">
            {session.agent_id} · {session.message_count} msgs
          </span>
          {session.parse_status !== "ok" && (
            <span className={`badge badge--${session.parse_status}`}>
              {session.parse_status}
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}
