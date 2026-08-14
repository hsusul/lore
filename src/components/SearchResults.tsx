import { Fragment } from "react";

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

export default function SearchResults({
  hits,
  query,
  selectedId,
  onOpen,
  hasMore,
  loadingMore,
  onLoadMore,
}: {
  hits: SearchHit[];
  query: string;
  selectedId: string | null;
  onOpen: (id: string) => void;
  hasMore: boolean;
  loadingMore: boolean;
  onLoadMore: () => void;
}) {
  if (query.trim() === "") return null;
  if (hits.length === 0) {
    return <p className="sessions__empty">No matches for “{query.trim()}”.</p>;
  }
  return (
    <>
      <ul className="results" role="listbox" aria-label="search results">
        {hits.map((hit, index) => (
          <li
            key={`${hit.source_id}-${index}`}
            role="option"
            aria-selected={hit.session_id === selectedId}
            className={`results__item${hit.session_id === selectedId ? " is-active" : ""}`}
            onClick={() => onOpen(hit.session_id)}
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
}
