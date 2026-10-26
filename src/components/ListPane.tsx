import type { ReactNode, RefObject } from "react";

import { agentLabel, formatRelative, formatTime } from "../format";
import type { SearchHit, SearchSort, SessionSummary } from "../ipc";
import type { PagedState } from "../hooks/usePagedList";
import { Snippet } from "./Highlight";
import ListBox, { type OpenVia } from "./ListBox";

const FIELD: Record<string, string> = {
  title: "title",
  text: "message",
  patch: "patch",
  content_json: "tool call",
};

function ParseFlag({ status }: { status: string }) {
  if (status === "ok") return null;
  return <span className={`flag ${status === "failed" ? "flag--danger" : "flag--warn"}`}>{status}</span>;
}

function SessionRow({ session }: { session: SessionSummary }) {
  return (
    <>
      <span className="row__title">{session.title ?? "Untitled session"}</span>
      <span className="row__meta">
        <span className={`agent agent--${session.agent_id}`}>{agentLabel(session.agent_id)}</span>
        <span>{session.message_count} messages</span>
        <ParseFlag status={session.parse_status} />
        <time className="row__time" title={formatTime(session.started_at)}>
          {formatRelative(session.started_at)}
        </time>
      </span>
    </>
  );
}

function HitRow({ hit }: { hit: SearchHit }) {
  return (
    <>
      <span className="row__title">{hit.title ?? "Untitled session"}</span>
      <span className="row__meta">
        <span className={`agent agent--${hit.agent_id}`}>{agentLabel(hit.agent_id)}</span>
        <span>in {FIELD[hit.field] ?? hit.field}</span>
        <time className="row__time" title={formatTime(hit.started_at)}>
          {formatRelative(hit.started_at)}
        </time>
      </span>
      <span className="row__snippet">
        <Snippet text={hit.snippet} />
      </span>
    </>
  );
}

function Placeholder({ role = "status", children }: { role?: "status" | "alert"; children: ReactNode }) {
  return (
    <div className="placeholder" role={role}>
      {children}
    </div>
  );
}

function MoreFooter({ cursor, more, onMore, label }: { cursor: string | null; more: string; onMore: () => void; label: string }) {
  if (cursor === null) return null;
  return (
    <div className="listbox__foot">
      <button type="button" className="btn btn--quiet" disabled={more === "loading"} onClick={onMore}>
        {more === "loading" ? "Loading…" : more === "failed" ? "Could not load more. Try again" : label}
      </button>
    </div>
  );
}

export function BrowsePane({
  heading,
  count,
  headerActions,
  state,
  openId,
  onOpen,
  onMore,
  onRetry,
  listRef,
  onExitTop,
  emptyHint,
}: {
  heading: string;
  count: number | null;
  headerActions?: ReactNode;
  state: PagedState<SessionSummary>;
  openId: string | null;
  onOpen: (id: string, via: OpenVia) => void;
  onMore: () => void;
  onRetry: () => void;
  listRef: RefObject<HTMLUListElement>;
  onExitTop: () => void;
  emptyHint: ReactNode;
}) {
  return (
    <section className="pane" aria-labelledby="pane-heading">
      <header className="pane__head">
        <h2 id="pane-heading" className="pane__title">
          {heading}
        </h2>
        {count !== null && <span className="count">{count}</span>}
        <span className="pane__spacer" />
        {headerActions}
      </header>
      {state.status === "loading" || state.status === "idle" ? (
        <Placeholder>Loading sessions…</Placeholder>
      ) : state.status === "failed" ? (
        <Placeholder role="alert">
          <p>Lore could not load these sessions.</p>
          <p className="quiet">{state.error}</p>
          <button type="button" className="btn" onClick={onRetry}>
            Try again
          </button>
        </Placeholder>
      ) : state.items.length === 0 ? (
        <Placeholder>{emptyHint}</Placeholder>
      ) : (
        <ListBox
          id="sessions"
          label={`Sessions in ${heading}`}
          items={state.items}
          keyOf={(s) => s.id}
          isOpen={(session) => session.id === openId}
          renderRow={(session) => <SessionRow session={session} />}
          onOpen={(session, via) => onOpen(session.id, via)}
          listRef={listRef}
          onExitTop={onExitTop}
          onReachEnd={onMore}
          footer={<MoreFooter cursor={state.cursor} more={state.more} onMore={onMore} label="Load older sessions" />}
        />
      )}
    </section>
  );
}

export function SearchPane({
  query,
  pending,
  needsTerm,
  scopeNote,
  sort,
  onSort,
  state,
  openId,
  onOpen,
  onMore,
  listRef,
  onExitTop,
}: {
  query: string;
  pending: boolean;
  needsTerm: boolean;
  scopeNote: ReactNode;
  sort: SearchSort;
  onSort: (sort: SearchSort) => void;
  state: PagedState<SearchHit>;
  openId: string | null;
  onOpen: (id: string, via: OpenVia) => void;
  onMore: () => void;
  listRef: RefObject<HTMLUListElement>;
  onExitTop: () => void;
}) {
  const ready = state.status === "ready" && !pending;
  const count = ready ? `${state.items.length}${state.cursor ? "+" : ""}` : null;
  return (
    <section className="pane" aria-labelledby="pane-heading">
      <header className="pane__head">
        <h2 id="pane-heading" className="pane__title">
          Results
        </h2>
        {count !== null && <span className="count">{count}</span>}
        <span className="pane__spacer" />
        <label className="sr-only" htmlFor="search-sort">
          Sort results
        </label>
        <select
          id="search-sort"
          className="select select--bare"
          value={sort}
          onChange={(event) => onSort(event.target.value as SearchSort)}
        >
          <option value="relevance">Best match</option>
          <option value="newest">Newest</option>
          <option value="oldest">Oldest</option>
        </select>
      </header>
      {scopeNote && <p className="pane__note">{scopeNote}</p>}
      {needsTerm ? (
        <Placeholder>
          <p>Filters narrow a search. Add a word to search for.</p>
          <p className="quiet">
            For example, <code>retry {query.trim()}</code>
          </p>
        </Placeholder>
      ) : pending || state.status === "loading" || state.status === "idle" ? (
        <Placeholder>Searching…</Placeholder>
      ) : state.status === "failed" ? (
        <Placeholder role="alert">
          <p>The search did not run.</p>
          <p className="quiet">{state.error}</p>
        </Placeholder>
      ) : state.items.length === 0 ? (
        <Placeholder>
          <p>No sessions match “{query.trim()}”.</p>
          <p className="quiet">Try fewer words, or drop a filter.</p>
        </Placeholder>
      ) : (
        <ListBox
          id="results"
          label="Search results"
          items={state.items}
          keyOf={(hit) => `${hit.source_kind}:${hit.source_id}:${hit.field}`}
          isOpen={(hit) => hit.session_id === openId}
          renderRow={(hit) => <HitRow hit={hit} />}
          onOpen={(hit, via) => onOpen(hit.session_id, via)}
          listRef={listRef}
          onExitTop={onExitTop}
          onReachEnd={onMore}
          footer={<MoreFooter cursor={state.cursor} more={state.more} onMore={onMore} label="Load more results" />}
        />
      )}
    </section>
  );
}
