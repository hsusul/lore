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

describe("SearchResults", () => {
  it("renders nothing for an empty query", () => {
    const { container } = render(
      <SearchResults hits={[]} query="   " selectedId={null} onOpen={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("shows an empty state when a query has no matches", () => {
    render(<SearchResults hits={[]} query="zzz" selectedId={null} onOpen={() => {}} />);
    expect(screen.getByText(/no matches/i)).toBeTruthy();
  });

  it("highlights matched terms and opens the session on click", () => {
    const onOpen = vi.fn();
    render(<SearchResults hits={[hit()]} query="retry" selectedId={null} onOpen={onOpen} />);

    // The matched term is wrapped in <mark>, and the markers themselves are gone.
    const mark = screen.getByText("retryBackoff");
    expect(mark.tagName).toBe("MARK");
    expect(screen.getByText("Add retry backoff")).toBeTruthy();
    expect(document.body.textContent).not.toContain(HIGHLIGHT_START);

    fireEvent.click(screen.getByRole("option"));
    expect(onOpen).toHaveBeenCalledWith("s1");
  });
});
