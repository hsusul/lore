import { useEffect, useRef, useState } from "react";

import { agentLabel } from "../format";
import { useDebounced } from "../hooks/useDebounced";
import {
  getSession,
  listSessionMessagesPage,
  listSessionsPage,
  searchPage,
  type MessageDto,
  type SessionDetail,
  type SessionSummary,
} from "../ipc";
import { shownTurn } from "../map/turns";
import { Snippet } from "./Highlight";
import { groupHits } from "./ListPane";

/** "Oct 16", as the prototype dates a session. */
const day = (ms: number | null) =>
  ms == null
    ? ""
    : new Date(ms).toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
      });

const minutes = (s: SessionSummary) =>
  s.started_at != null && s.ended_at != null
    ? Math.max(1, Math.round((s.ended_at - s.started_at) / 60_000))
    : null;

/** One row of the list: an archived session, or the session a search hit is in. */
interface Row {
  id: string;
  title: string;
  agent: string;
  at: number | null;
  /** A search snippet with highlight markers, or the number of messages. */
  snippet: string | null;
  meta: string;
}

const fromSession = (s: SessionSummary): Row => ({
  id: s.id,
  title: s.title || "Untitled session",
  agent: s.agent_id,
  at: s.started_at,
  snippet: null,
  meta: `${agentLabel(s.agent_id)}, ${s.message_count} messages`,
});

type List =
  | { status: "loading" }
  | { status: "failed"; error: string }
  | { status: "ready"; rows: Row[]; cursor: string | null; more: boolean };

const PAGE = 50;

/**
 * The archive inside the map's card: a search field over every session, and
 * the newest sessions when it is empty. Opening one shows its conversation in
 * the same card.
 */
export function SessionsList({
  onOpen,
  focusSearch,
}: {
  onOpen: (id: string) => void;
  focusSearch: number;
}) {
  const [query, setQuery] = useState("");
  const settled = useDebounced(query, 250);
  const [list, setList] = useState<List>({ status: "loading" });
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (focusSearch) input.current?.focus();
  }, [focusSearch]);

  const fetchPage = async (
    cursor: string | null,
  ): Promise<{ rows: Row[]; cursor: string | null }> => {
    if (settled.trim()) {
      const page = await searchPage(settled.trim(), PAGE, cursor);
      const rows = groupHits(page.hits).map(({ hit, snippet, hits }) => ({
        id: hit.session_id,
        title: hit.title || "Untitled session",
        agent: hit.agent_id,
        at: hit.started_at,
        snippet: snippet.field === "title" ? null : snippet.snippet,
        meta: `${agentLabel(hit.agent_id)}${hits > 1 ? `, ${hits} matches` : ""}`,
      }));
      return { rows, cursor: page.next_cursor };
    }
    const page = await listSessionsPage(PAGE, cursor);
    return { rows: page.sessions.map(fromSession), cursor: page.next_cursor };
  };

  useEffect(() => {
    let alive = true;
    setList({ status: "loading" });
    fetchPage(null).then(
      (page) =>
        alive &&
        setList({
          status: "ready",
          rows: page.rows,
          cursor: page.cursor,
          more: false,
        }),
      (e: unknown) => alive && setList({ status: "failed", error: String(e) }),
    );
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- a new query starts a new list
  }, [settled]);

  const loadMore = async () => {
    if (list.status !== "ready" || !list.cursor) return;
    setList({ ...list, more: true });
    try {
      const page = await fetchPage(list.cursor);
      const seen = new Set(list.rows.map((r) => r.id));
      setList({
        status: "ready",
        rows: [...list.rows, ...page.rows.filter((r) => !seen.has(r.id))],
        cursor: page.cursor,
        more: false,
      });
    } catch (e) {
      setList({ status: "failed", error: String(e) });
    }
  };

  return (
    <>
      <div className="mist-pin">
        <div className="mist-kind">Sessions</div>
        <label className="mist-qbar">
          <svg
            viewBox="0 0 24 24"
            width="16"
            height="16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
            aria-hidden="true"
          >
            <circle cx="11" cy="11" r="6" />
            <path d="M20 20l-4-4" />
          </svg>
          <input
            ref={input}
            type="search"
            aria-label="Search sessions"
            placeholder="Search"
            spellCheck={false}
            autoComplete="off"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
      </div>
      {list.status === "loading" && <p className="mist-mt">Reading…</p>}
      {list.status === "failed" && (
        <p className="mist-mt" role="alert">
          {list.error}
        </p>
      )}
      {list.status === "ready" && (
        <>
          {list.rows.length === 0 && (
            <p className="mist-mt">
              {settled.trim() ? "No sessions match." : "No sessions yet."}
            </p>
          )}
          <ul className="mist-list">
            {list.rows.map((row) => (
              <li key={row.id}>
                <button
                  type="button"
                  className="mist-it"
                  onClick={() => onOpen(row.id)}
                >
                  <span className="mist-dot" />
                  <span className="mist-tx">
                    <span className="mist-hd">
                      <span className="mist-t">{row.title}</span>
                      <span className="mist-mt">{day(row.at)}</span>
                    </span>
                    {row.snippet && (
                      <span className="mist-sn">
                        <Snippet text={row.snippet} />
                      </span>
                    )}
                    <span className="mist-mt">{row.meta}</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
          {list.cursor && (
            <button
              type="button"
              className="mist-btn"
              disabled={list.more}
              onClick={() => void loadMore()}
            >
              {list.more ? "Loading…" : "Show more"}
            </button>
          )}
        </>
      )}
    </>
  );
}

/** What a turn shows: its text, or the tool it ran in one quiet line. */
function turnText(m: MessageDto): { text: string; tool: boolean } | null {
  const text = m.parts
    .filter((p) => p.kind === "text" && p.text)
    .map((p) => p.text)
    .join("\n\n")
    .trim();
  if (text) {
    const shown = shownTurn(text);
    return shown && { text: shown.text, tool: shown.quiet };
  }
  const call = m.parts.find((p) => p.kind === "tool_use");
  if (!call?.content_json) return null;
  try {
    const name = (JSON.parse(call.content_json) as { name?: string }).name;
    return name ? { text: `Ran ${name}`, tool: true } : null;
  } catch {
    return null;
  }
}

function who(m: MessageDto, agent: string): string {
  if (m.role === "user") return "You";
  if (m.role === "assistant") return agentLabel(agent);
  return m.role === "tool" ? "Tool" : "Note";
}

type Opened =
  | { status: "loading" }
  | { status: "failed"; error: string }
  | {
      status: "ready";
      detail: SessionDetail;
      messages: MessageDto[];
      cursor: string | null;
      more: boolean;
    };

/** One session's conversation, in the card, with a way back to the list. */
export function SessionView({
  id,
  onBack,
}: {
  id: string;
  onBack: () => void;
}) {
  const [opened, setOpened] = useState<Opened>({ status: "loading" });

  useEffect(() => {
    let alive = true;
    setOpened({ status: "loading" });
    getSession(id).then(
      (detail) => {
        if (!alive) return;
        if (!detail)
          setOpened({
            status: "failed",
            error: "This session is no longer in the archive.",
          });
        else
          setOpened({
            status: "ready",
            detail,
            messages: detail.messages,
            cursor: detail.next_message_cursor,
            more: false,
          });
      },
      (e: unknown) =>
        alive && setOpened({ status: "failed", error: String(e) }),
    );
    return () => {
      alive = false;
    };
  }, [id]);

  const loadMore = async () => {
    if (opened.status !== "ready" || !opened.cursor) return;
    setOpened({ ...opened, more: true });
    try {
      const page = await listSessionMessagesPage(id, 200, opened.cursor);
      setOpened({
        ...opened,
        messages: [...opened.messages, ...page.messages],
        cursor: page.next_cursor,
        more: false,
      });
    } catch (e) {
      setOpened({ status: "failed", error: String(e) });
    }
  };

  return (
    <>
      <button type="button" className="mist-back" onClick={onBack}>
        ‹ Sessions
      </button>
      {opened.status === "loading" && <p className="mist-mt">Reading…</p>}
      {opened.status === "failed" && (
        <p className="mist-mt" role="alert">
          {opened.error}
        </p>
      )}
      {opened.status === "ready" && (
        <article aria-label={opened.detail.summary.title || "Untitled session"}>
          <div className="mist-kind">
            {[
              agentLabel(opened.detail.summary.agent_id),
              day(opened.detail.summary.started_at),
              minutes(opened.detail.summary) &&
                `${minutes(opened.detail.summary)} min`,
            ]
              .filter(Boolean)
              .join(", ")}
          </div>
          <h2 className="mist-h2">
            {opened.detail.summary.title || "Untitled session"}
          </h2>
          <div className="mist-turns">
            {opened.messages.map((m) => {
              const shown = turnText(m);
              if (!shown || m.is_sidechain) return null;
              return (
                <div
                  key={m.id}
                  className={`mist-turn${m.role === "user" ? " you" : ""}${shown.tool ? " tool" : ""}`}
                >
                  <span className="mist-dot" />
                  <div>
                    <div className="mist-w">
                      {who(m, opened.detail.summary.agent_id)}
                    </div>
                    <p>{shown.text}</p>
                  </div>
                </div>
              );
            })}
          </div>
          {opened.cursor && (
            <button
              type="button"
              className="mist-btn"
              disabled={opened.more}
              onClick={() => void loadMore()}
            >
              {opened.more ? "Loading…" : "Show more"}
            </button>
          )}
        </article>
      )}
    </>
  );
}
