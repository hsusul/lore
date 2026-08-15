import { forwardRef, Fragment, useEffect, useRef, useState } from "react";

import { agentLabel } from "../format";
import { HIGHLIGHT_END, HIGHLIGHT_START, type SearchHit } from "../ipc";

/** Split a snippet on the highlight markers into plain/highlighted runs. */
function Snippet({ text }: { text: string }) {
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
  const activeRef = useRef<HTMLLIElement>(null);
  const last = hits.length - 1;
  const active =
    navigation.query === query
      ? Math.min(navigation.index, Math.max(last, 0))
      : 0;
  const selectedIndex =
    selectedId === null ? -1 : hits.findIndex((hit) => hit.session_id === selectedId);

  // Keep the active row visible during keyboard navigation. `block: "nearest"`
  // scrolls the results pane minimally and copes with variable-height rows
  // (snippets wrap), so no fixed-height windowing is assumed.
  useEffect(() => {
    // Optional call: scrollIntoView is absent in some test environments (jsdom).
    activeRef.current?.scrollIntoView?.({ block: "nearest" });
  }, [active, hits.length]);

  function onKeyDown(event: React.KeyboardEvent<HTMLUListElement>) {
    if (event.key === "ArrowDown" || event.key === "j") {
      event.preventDefault();
      setNavigation({ query, index: Math.min(active + 1, last) });
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (active === 0 && onExitUp) onExitUp();
      else setNavigation({ query, index: Math.max(active - 1, 0) });
    } else if (event.key === "k") {
      event.preventDefault();
      setNavigation({ query, index: Math.max(active - 1, 0) });
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
    return <p className="sessions__empty">No matches for “{query.trim()}”.</p>;
  }
  return (
    <>
      <ul
        ref={forwardedRef}
        className="results"
        role="listbox"
        aria-label="search results"
        tabIndex={0}
        aria-activedescendant={`search-result-${active}`}
        onKeyDown={onKeyDown}
      >
        {hits.map((hit, index) => (
          <li
            key={`${hit.source_kind}-${hit.source_id}-${hit.field}`}
            id={`search-result-${index}`}
            ref={index === active ? activeRef : undefined}
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
        ))}
      </ul>
      {hasMore ? (
        <button
          type="button"
          className="results__more"
          onClick={onLoadMore}
          disabled={loadingMore}
        >
          {loadingMore ? "Loading…" : "Load more results"}
        </button>
      ) : null}
    </>
  );
});

export default SearchResults;
