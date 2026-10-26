import { clsx } from "clsx";
import { ArrowDownUp, Loader } from "lucide-react";
import { useEffect, useState, type ReactNode, type RefObject } from "react";

import { agentLabel, formatMailDate, formatTime } from "../format";
import { HIGHLIGHT_END, HIGHLIGHT_START, type SearchHit, type SearchSort, type SessionSummary } from "../ipc";
import type { PagedState } from "../hooks/usePagedList";
import { Snippet } from "./Highlight";
import ListBox, { type MoreState, type OpenVia } from "./ListBox";
import { SORTS } from "./SearchField";
import { Button, ICON } from "./ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { Tooltip } from "./ui/tooltip";

const PARSE_NOTES: Record<string, string> = { partial: "Partially parsed", failed: "Not parsed" };

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Line 1 of every row: the title, and the date trailing as Mail writes it. */
function RowHead({ title, startedAt }: { title: string; startedAt: number | null }) {
  return (
    <span className="flex min-w-0 items-baseline gap-2">
      <span className="min-w-0 flex-1 truncate text-headline">{title}</span>
      <time className="row__secondary flex-none text-subheadline tabular-nums" title={formatTime(startedAt)}>
        {formatMailDate(startedAt)}
      </time>
    </span>
  );
}

/** Secondary lines: one run of callout text, one line, ellipsis. */
function RowLine({ children }: { children: ReactNode }) {
  return <span className="row__secondary truncate text-callout">{children}</span>;
}

function SessionRow({ session }: { session: SessionSummary }) {
  const note = PARSE_NOTES[session.parse_status];
  return (
    <>
      <RowHead title={session.title ?? "Untitled session"} startedAt={session.started_at} />
      <RowLine>
        {agentLabel(session.agent_id)}, {plural(session.message_count, "message")}
        {note && `, ${note}`}
      </RowLine>
    </>
  );
}

/**
 * One session among the search results: its best hit, the hit whose snippet the row
 * shows, and how many hits the loaded pages hold for it.
 */
interface ResultGroup {
  hit: SearchHit;
  snippet: SearchHit;
  hits: number;
}

/**
 * One row per session, in the order of each session's best hit (spec 9.2, 17.14). A
 * title match would only repeat line 1, so the snippet comes from the best other hit.
 */
function groupHits(hits: SearchHit[]): ResultGroup[] {
  const groups = new Map<string, ResultGroup>();
  for (const hit of hits) {
    const group = groups.get(hit.session_id);
    if (!group) groups.set(hit.session_id, { hit, snippet: hit, hits: 1 });
    else {
      group.hits += 1;
      if (group.snippet.field === "title" && hit.field !== "title") group.snippet = hit;
    }
  }
  return [...groups.values()];
}

function plainSnippet(snippet: string): string {
  return snippet.split(HIGHLIGHT_START).join("").split(HIGHLIGHT_END).join("").trim();
}

function HitRow({ group, complete }: { group: ResultGroup; complete: boolean }) {
  const { hit, snippet, hits } = group;
  const more = hits > 1 ? (complete ? plural(hits - 1, "more match", "more matches") : "More matches in this session") : null;
  return (
    <>
      <RowHead title={hit.title ?? "Untitled session"} startedAt={hit.started_at} />
      <RowLine>
        <Snippet text={snippet.snippet} />
      </RowLine>
      <RowLine>{more}</RowLine>
    </>
  );
}

/** A filter-only result: the session itself, from what a hit carries (spec 18.7). */
function FilterRow({ hit }: { hit: SearchHit }) {
  return (
    <>
      <RowHead title={hit.title ?? (plainSnippet(hit.snippet) || "Untitled session")} startedAt={hit.started_at} />
      <RowLine>{agentLabel(hit.agent_id)}</RowLine>
    </>
  );
}

/** Nothing for the first second, then a 15px spinner centered in the list (spec 10). */
function Loading() {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setShown(true), 1000);
    return () => clearTimeout(timer);
  }, []);
  return (
    <div role="status" aria-label="Loading" className="flex flex-1 items-center justify-center">
      {shown && <Loader className={clsx(ICON.body, "animate-spin text-label-2")} aria-hidden="true" />}
    </div>
  );
}

/** A centered list state: a headline, then a line or two and push buttons. */
function ListState({ role = "status", title, children }: { role?: "status" | "alert"; title: string; children?: ReactNode }) {
  return (
    <div role={role} className="flex flex-1 flex-col items-center justify-center gap-1 px-6 text-center">
      <p className="text-headline text-label-2">{title}</p>
      {children}
    </div>
  );
}

function moreState(state: PagedState<unknown>, label: string): MoreState | null {
  return state.status === "ready" && state.cursor !== null ? { label, status: state.more } : null;
}

/** The session list for a scope. Its title and count live in the toolbar above it. */
export function BrowsePane({
  label,
  state,
  openId,
  onOpen,
  onMore,
  onRetry,
  listRef,
  onExitTop,
  emptyHint,
}: {
  label: string;
  state: PagedState<SessionSummary>;
  openId: string | null;
  onOpen: (id: string, via: OpenVia) => void;
  onMore: () => void;
  onRetry: () => void;
  listRef: RefObject<HTMLUListElement>;
  onExitTop: () => void;
  emptyHint: string;
}) {
  return (
    <section className="pane" aria-label={label}>
      {state.status === "loading" || state.status === "idle" ? (
        <Loading />
      ) : state.status === "failed" ? (
        <ListState role="alert" title="Lore could not load these sessions.">
          <p className="text-callout text-label-2">{state.error}</p>
          <Button className="mt-2" onClick={onRetry}>
            Try again
          </Button>
        </ListState>
      ) : state.items.length === 0 ? (
        <ListState title={emptyHint} />
      ) : (
        <ListBox
          id="sessions"
          label={`Sessions in ${label}`}
          items={state.items}
          keyOf={(s) => s.id}
          isOpen={(session) => session.id === openId}
          renderRow={(session) => <SessionRow session={session} />}
          onOpen={(session, via) => onOpen(session.id, via)}
          listRef={listRef}
          height="row"
          onExitTop={onExitTop}
          onReachEnd={onMore}
          more={moreState(state, "Load older sessions")}
        />
      )}
    </section>
  );
}

/** The sort order as a compact menu at the end of the scope bar (spec 9.2). */
function SortMenu({ sort, onSort }: { sort: SearchSort; onSort: (sort: SearchSort) => void }) {
  const label = SORTS.find((option) => option.value === sort)?.label;
  return (
    <DropdownMenu>
      <Tooltip label={`Sort by ${label}`}>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={`Sort results, ${label}`}
            className="flex size-control flex-none items-center justify-center rounded-control text-label-2 outline-none hover:bg-fill-2 focus-visible:shadow-focus"
          >
            <ArrowDownUp className={ICON.body} aria-hidden="true" />
          </button>
        </DropdownMenuTrigger>
      </Tooltip>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>Sort By</DropdownMenuLabel>
        <DropdownMenuRadioGroup value={sort} onValueChange={(value) => onSort(value as SearchSort)}>
          {SORTS.map((option) => (
            <DropdownMenuRadioItem key={option.value} value={option.value}>
              {option.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Mail's scope bar under the toolbar while searching: "Search: All Sessions | <scope>". */
export function ScopeBar({
  scope,
  onSearchAll,
  sort,
  onSort,
}: {
  /** The scope the search runs in, or null for everything; `narrows` is false for a folder. */
  scope: { label: string; narrows: boolean } | null;
  onSearchAll: () => void;
  sort: SearchSort;
  onSort: (sort: SearchSort) => void;
}) {
  const scoped = scope?.narrows === true;
  const option = "flex h-control items-center whitespace-nowrap rounded-control px-2 text-label outline-none focus-visible:shadow-focus aria-checked:bg-fill aria-checked:font-semibold";
  return (
    <div className="flex flex-none items-center gap-1 pb-1.5 pl-5 pr-2.5 pt-2 text-callout text-label-2">
      <span id="search-scope">Search:</span>
      <div role="radiogroup" aria-labelledby="search-scope" className="flex min-w-0 items-center gap-1">
        <button type="button" role="radio" aria-checked={!scoped} className={clsx(option, "flex-none")} onClick={onSearchAll}>
          All Sessions
        </button>
        {scope && (
          <button
            type="button"
            role="radio"
            aria-checked={scoped}
            disabled={!scope.narrows}
            title={scope.narrows ? undefined : "A folder cannot narrow a search yet"}
            className={clsx(option, "min-w-0 truncate disabled:text-label-3")}
          >
            {scope.label}
          </button>
        )}
      </div>
      <span className="flex-1" />
      <SortMenu sort={sort} onSort={onSort} />
    </div>
  );
}

export function SearchPane({
  query,
  terms,
  filters,
  pending,
  scopeBar,
  state,
  openId,
  onOpen,
  onMore,
  listRef,
  onExitTop,
  onSearchAll,
  onClearFilters,
}: {
  query: string;
  terms: string[];
  filters: string[];
  pending: boolean;
  scopeBar: ReactNode;
  state: PagedState<SearchHit>;
  openId: string | null;
  onOpen: (id: string, via: OpenVia) => void;
  onMore: () => void;
  listRef: RefObject<HTMLUListElement>;
  onExitTop: () => void;
  /** Widen a scoped search to every session; null when the search is not scoped. */
  onSearchAll: (() => void) | null;
  onClearFilters: () => void;
}) {
  const filterOnly = terms.length === 0;
  const groups = state.status === "ready" ? groupHits(state.items) : [];
  const complete = state.status === "ready" && state.cursor === null;
  return (
    <section className="pane" aria-label="Search results">
      {scopeBar}
      {pending || state.status === "loading" || state.status === "idle" ? (
        <Loading />
      ) : state.status === "failed" ? (
        <ListState role="alert" title="The search did not run.">
          <p className="text-callout text-label-2">{state.error}</p>
        </ListState>
      ) : groups.length === 0 ? (
        <ListState title="No Results">
          <p className="text-callout text-label-2">
            {terms.length > 0 && `“${terms.join(" ")}”`}
            {terms.length > 0 && filters.length > 0 && " with "}
            {filters.length > 0 && <span className="font-mono">{filters.join(" ")}</span>}
            {terms.length === 0 && filters.length === 0 && `“${query.trim()}”`}
          </p>
          {(onSearchAll || filters.length > 0) && (
            <div className="mt-2 flex flex-col gap-2">
              {onSearchAll && <Button onClick={onSearchAll}>Search All Sessions</Button>}
              {filters.length > 0 && <Button onClick={onClearFilters}>Clear Filters</Button>}
            </div>
          )}
        </ListState>
      ) : (
        <ListBox
          id="results"
          label="Search results"
          items={groups}
          keyOf={(group) => group.hit.session_id}
          isOpen={(group) => group.hit.session_id === openId}
          renderRow={(group) => (filterOnly ? <FilterRow hit={group.hit} /> : <HitRow group={group} complete={complete} />)}
          onOpen={(group, via) => onOpen(group.hit.session_id, via)}
          listRef={listRef}
          height={filterOnly ? "row" : "hit-row"}
          onExitTop={onExitTop}
          onReachEnd={onMore}
          more={moreState(state, "Load more results")}
        />
      )}
    </section>
  );
}
