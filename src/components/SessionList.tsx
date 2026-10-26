import { useEffect, useRef, useState } from "react";

import { agentLabel, clamp, formatRelative } from "../format";
import type { SessionSummary } from "../ipc";
import { useWindowing } from "../virtual";

/** Drag payload MIME for filing a thread into a folder via drag-and-drop. */
export const SESSION_DND_MIME = "application/x-lore-session";

/**
 * A keyboard-navigable session list (listbox pattern): Arrow/j/k move the active
 * row, Home/End jump to the ends, Enter opens it, click selects and opens.
 */
export default function SessionList({
  sessions,
  selectedId,
  onOpen,
  hasMore = false,
  loadingMore = false,
  onLoadMore,
}: {
  sessions: SessionSummary[];
  selectedId: string | null;
  onOpen: (id: string) => void;
  hasMore?: boolean;
  loadingMore?: boolean;
  onLoadMore?: () => void;
}) {
  const listKey = sessions[0]?.id ?? null;
  const [navigation, setNavigation] = useState({ listKey, index: 0 });
  const active =
    navigation.listKey === listKey
      ? clamp(navigation.index, 0, Math.max(sessions.length - 1, 0))
      : 0;
  const listRef = useRef<HTMLUListElement>(null);
  const { startIndex, endIndex, padTop, padBottom, scrollToIndex } = useWindowing(
    listRef,
    sessions.length,
  );

  // Keep the active row visible during keyboard navigation. Driving the scroll
  // by index (rather than querying the element) works even when the active row
  // is outside the currently rendered window.
  useEffect(() => {
    scrollToIndex(active);
  }, [active, scrollToIndex]);

  function onKeyDown(event: React.KeyboardEvent<HTMLUListElement>) {
    const last = sessions.length - 1;
    if (event.key === "ArrowDown" || event.key === "j") {
      event.preventDefault();
      setNavigation({ listKey, index: Math.min(active + 1, last) });
    } else if (event.key === "ArrowUp" || event.key === "k") {
      event.preventDefault();
      setNavigation({ listKey, index: Math.max(active - 1, 0) });
    } else if (event.key === "Home") {
      event.preventDefault();
      setNavigation({ listKey, index: 0 });
    } else if (event.key === "End") {
      event.preventDefault();
      setNavigation({ listKey, index: last });
    } else if (event.key === "Enter") {
      event.preventDefault();
      const session = sessions[active];
      if (session) onOpen(session.id);
    }
  }

  if (sessions.length === 0) {
    return (
      <div className="sessions__empty">
        <p>No sessions yet.</p>
        <p className="empty">
          Run a <strong>Rescan</strong> to ingest your agent history, or press{" "}
          <kbd>⌘</kbd>
          <kbd>K</kbd> to jump around.
        </p>
      </div>
    );
  }

  return (
    <>
      <ul
        ref={listRef}
        className="sessions"
        role="listbox"
        aria-label="Sessions"
        tabIndex={0}
        aria-activedescendant={sessions[active]?.id}
        onKeyDown={onKeyDown}
      >
        {padTop > 0 && <li aria-hidden="true" style={{ height: padTop }} />}
        {sessions.slice(startIndex, endIndex).map((session, offset) => {
          const index = startIndex + offset;
          return (
            <li
              key={session.id}
              id={session.id}
              data-vrow
              role="option"
              aria-selected={session.id === selectedId}
              aria-setsize={sessions.length}
              aria-posinset={index + 1}
              className={`sessions__item${index === active ? " is-active" : ""}`}
              draggable
              onDragStart={(event) => {
                event.dataTransfer.setData(SESSION_DND_MIME, session.id);
                event.dataTransfer.effectAllowed = "move";
              }}
              onClick={() => {
                setNavigation({ listKey, index });
                onOpen(session.id);
              }}
            >
              <span className="sessions__title">{session.title ?? "(untitled)"}</span>
              {session.parse_status !== "ok" && (
                <span
                  className={`chip ${session.parse_status === "failed" ? "chip--danger" : "chip--warn"}`}
                >
                  {session.parse_status}
                </span>
              )}
              <span className="sessions__meta">
                <span className="chip chip--agent">{agentLabel(session.agent_id)}</span>
                <span>{session.message_count} msgs</span>
                <span className="sessions__time">{formatRelative(session.started_at)}</span>
              </span>
            </li>
          );
        })}
        {padBottom > 0 && <li aria-hidden="true" style={{ height: padBottom }} />}
      </ul>
      {hasMore ? (
        <div className="sessions__pagination" role="status" aria-live="polite">
          <button
            type="button"
            className="sessions__more"
            onClick={onLoadMore}
            disabled={loadingMore}
          >
            {loadingMore ? "Loading older sessions…" : "Load older sessions"}
          </button>
        </div>
      ) : null}
    </>
  );
}
