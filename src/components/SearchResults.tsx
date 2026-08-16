import { forwardRef, Fragment, useCallback, useEffect, useRef, useState } from "react";

import { agentLabel } from "../format";
import { HIGHLIGHT_END, HIGHLIGHT_START, type SearchHit } from "../ipc";
import { useWindowing } from "../virtual";

/** Split a snippet on the highlight markers into plain/highlighted runs. */
function Snippet({ text }: { text: string }) {
  if (!text.includes(HIGHLIGHT_START)) {
    return <span className="hit__snippet">{text}</span>;
  }
  const nodes: React.ReactNode[] = [];
  let rest = text;
  let key = 0;
  while (rest.length > 0) {
    const start = rest.indexOf(HIGHLIGHT_START);
    if (start === -1) {
      nodes.push(<Fragment key={key++}>{rest}</Fragment>);
      break;
    }
    if (start > 0) nodes.push(<Fragment key={key++}>{rest.slice(0, start)}</Fragment>);
    const end = rest.indexOf(HIGHLIGHT_END, start + HIGHLIGHT_START.length);
    if (end === -1) {
      nodes.push(<Fragment key={key++}>{rest.slice(start + HIGHLIGHT_START.length)}</Fragment>);
      break;
    }
    const matched = rest.slice(start + HIGHLIGHT_START.length, end);
    nodes.push(<mark key={key++}>{matched}</mark>);
    rest = rest.slice(end + HIGHLIGHT_END.length);
  }
  return <span className="hit__snippet">{nodes}</span>;
}

const FIELD_LABEL: Record<string, string> = {
  title: "title",
  text: "message",
  patch: "patch",
  content_json: "tool",
};

interface SearchResultsProps {
  hits: SearchHit[];
  query: string;
  selectedId: string | null;
  onOpen: (id: string) => void;
  searching: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  onLoadMore: () => void;
  onExitUp?: () => void;
}

const SearchResults = forwardRef<HTMLUListElement, SearchResultsProps>(function SearchResults({
  hits,
  query,
  selectedId,
  onOpen,
  searching,
  hasMore,
  loadingMore,
  onLoadMore,
  onExitUp,
}, forwardedRef) {
  // Keyboard-navigable listbox, mirroring SessionList: Arrow/j/k move the active
  // row, Home/End jump to the ends, Enter opens it. Hooks run unconditionally
  // (before the early returns below) to satisfy the Rules of Hooks.
  const [navigation, setNavigation] = useState({ query, index: 0 });
  const listRef = useRef<HTMLUListElement | null>(null);
  const last = hits.length - 1;
  const active =
    navigation.query === query
      ? Math.min(navigation.index, Math.max(last, 0))
      : 0;
  const selectedIndex =
    selectedId === null ? -1 : hits.findIndex((hit) => hit.session_id === selectedId);

  // Window the (potentially large, "Load more"-accumulated) result list so only
  // the visible rows are in the DOM — rows are fixed-height (the snippet is
  // clamped in CSS), so the same primitive as SessionList applies. In test
  // environments without layout it degrades to rendering every row.
  const { startIndex, endIndex, padTop, padBottom, scrollToIndex } = useWindowing(
    listRef,
    hits.length,
  );

  // Expose the list element to the parent (App focuses it on ArrowDown) while
  // still holding a local ref for windowing/measurement.
  const setListRef = useCallback(
    (el: HTMLUListElement | null) => {
      listRef.current = el;
      if (typeof forwardedRef === "function") forwardedRef(el);
      else if (forwardedRef) forwardedRef.current = el;
    },
    [forwardedRef],
  );

  // Keep the active row visible during keyboard navigation. Driving the scroll
  // by index works even when the active row is outside the rendered window.
  useEffect(() => {
    scrollToIndex(active);
  }, [active, scrollToIndex]);

  function onKeyDown(event: React.KeyboardEvent<HTMLUListElement>) {
    if (event.key === "ArrowDown" || event.key === "j") {
      event.preventDefault();
      setNavigation({ query, index: Math.min(active + 1, last) });
    } else if (event.key === "ArrowUp" || event.key === "k") {
      event.preventDefault();
      if (active === 0 && onExitUp) onExitUp();
      else setNavigation({ query, index: Math.max(active - 1, 0) });
    } else if (event.key === "Home") {
      event.preventDefault();
      setNavigation({ query, index: 0 });
    } else if (event.key === "End") {
      event.preventDefault();
      setNavigation({ query, index: Math.max(last, 0) });
    } else if (event.key === "Enter") {
      event.preventDefault();
      const hit = hits[active];
      if (hit) onOpen(hit.session_id);
    }
  }

  if (query.trim() === "") return null;
  if (searching && hits.length === 0) {
    return (
      <p className="sessions__empty" role="status">
        Searching…
      </p>
    );
  }
  if (hits.length === 0) {
    return (
      <p className="sessions__empty" role="status">
        No matches for “{query.trim()}”.
      </p>
    );
  }
  return (
    <>
      <ul
        ref={setListRef}
        className="results"
        role="listbox"
        aria-label="Search results"
        tabIndex={0}
        aria-activedescendant={`search-result-${active}`}
        onKeyDown={onKeyDown}
      >
        {padTop > 0 && <li aria-hidden="true" style={{ height: padTop }} />}
        {hits.slice(startIndex, endIndex).map((hit, offset) => {
          const index = startIndex + offset;
          return (
            <li
              key={`${hit.source_kind}-${hit.source_id}-${hit.field}`}
              id={`search-result-${index}`}
              data-vrow
              role="option"
              aria-selected={index === selectedIndex}
              aria-setsize={hits.length}
              aria-posinset={index + 1}
              className={`results__item${index === active ? " is-active" : ""}`}
              onClick={() => {
                setNavigation({ query, index });
                onOpen(hit.session_id);
              }}
            >
              <div className="results__meta">
                <span className="results__title">{hit.title ?? "(untitled)"}</span>
                <span className="chip chip--agent">{agentLabel(hit.agent_id)}</span>
                <span className="results__field">{FIELD_LABEL[hit.field] ?? hit.field}</span>
              </div>
              <Snippet text={hit.snippet} />
            </li>
          );
        })}
        {padBottom > 0 && <li aria-hidden="true" style={{ height: padBottom }} />}
      </ul>
      {hasMore ? (
        <div className="sessions__pagination" role="status" aria-live="polite">
          <button
            type="button"
            className="results__more"
            onClick={onLoadMore}
            disabled={loadingMore}
            aria-busy={loadingMore}
          >
            {loadingMore ? "Loading…" : "Load more results"}
          </button>
        </div>
      ) : null}
    </>
  );
});

export default SearchResults;
