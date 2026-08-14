import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import SearchResults from "./SearchResults";
import { HIGHLIGHT_END, HIGHLIGHT_START, type SearchHit } from "../ipc";

function hit(overrides: Partial<SearchHit> = {}): SearchHit {
  return {
    session_id: "s1",
    source_kind: "message_part",
    source_id: "p1",
    field: "text",
    snippet: `deploy the ${HIGHLIGHT_START}retryBackoff${HIGHLIGHT_END} helper`,
    rank: -1.2,
    title: "Add retry backoff",
    agent_id: "codex",
    started_at: null,
    ...overrides,
  };
}

const noPaging = { hasMore: false, loadingMore: false, onLoadMore: () => {} };

describe("SearchResults", () => {
  it("renders nothing for an empty query", () => {
    const { container } = render(
      <SearchResults hits={[]} query="   " selectedId={null} onOpen={() => {}} {...noPaging} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("shows an empty state when a query has no matches", () => {
    render(
      <SearchResults hits={[]} query="zzz" selectedId={null} onOpen={() => {}} {...noPaging} />,
    );
    expect(screen.getByText(/no matches/i)).toBeTruthy();
  });

  it("highlights matched terms and opens the session on click", () => {
    const onOpen = vi.fn();
    render(
      <SearchResults hits={[hit()]} query="retry" selectedId={null} onOpen={onOpen} {...noPaging} />,
    );

    // The matched term is wrapped in <mark>, and the markers themselves are gone.
    const mark = screen.getByText("retryBackoff");
    expect(mark.tagName).toBe("MARK");
    expect(screen.getByText("Add retry backoff")).toBeTruthy();
    expect(document.body.textContent).not.toContain(HIGHLIGHT_START);

    fireEvent.click(screen.getByRole("option"));
    expect(onOpen).toHaveBeenCalledWith("s1");
  });

  it("shows a Load more control only when more pages exist and fires it on click", () => {
    const onLoadMore = vi.fn();
    const { rerender } = render(
      <SearchResults hits={[hit()]} query="retry" selectedId={null} onOpen={() => {}} {...noPaging} />,
    );
    // No further pages: no button.
    expect(screen.queryByRole("button", { name: /load more/i })).toBeNull();

    // More pages available: the button appears and invokes the callback.
    rerender(
      <SearchResults
        hits={[hit()]}
        query="retry"
        selectedId={null}
        onOpen={() => {}}
        hasMore
        loadingMore={false}
        onLoadMore={onLoadMore}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /load more/i }));
    expect(onLoadMore).toHaveBeenCalledTimes(1);

    // While a page is loading the control is disabled to prevent double fetches.
    rerender(
      <SearchResults
        hits={[hit()]}
        query="retry"
        selectedId={null}
        onOpen={() => {}}
        hasMore
        loadingMore
        onLoadMore={onLoadMore}
      />,
    );
    expect(screen.getByRole("button", { name: /loading/i }).hasAttribute("disabled")).toBe(true);
  });
});
