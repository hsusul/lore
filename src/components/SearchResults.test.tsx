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

const idle = {
  searching: false,
  hasMore: false,
  loadingMore: false,
  onLoadMore: () => {},
};

describe("SearchResults", () => {
  it("renders nothing for an empty query", () => {
    const { container } = render(
      <SearchResults hits={[]} query="   " selectedId={null} onOpen={() => {}} {...idle} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("shows an empty state when a query has no matches", () => {
    render(
      <SearchResults hits={[]} query="zzz" selectedId={null} onOpen={() => {}} {...idle} />,
    );
    expect(screen.getByText(/no matches/i)).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("No matches");
  });

  it("announces an in-flight search instead of showing a false empty state", () => {
    render(
      <SearchResults
        hits={[]}
        query="retry"
        selectedId={null}
        onOpen={() => {}}
        {...idle}
        searching
      />,
    );
    expect(screen.getByRole("status").textContent).toContain("Searching");
    expect(screen.queryByText(/no matches/i)).toBeNull();
  });

  it("highlights matched terms and opens the session on click", () => {
    const onOpen = vi.fn();
    render(
      <SearchResults hits={[hit()]} query="retry" selectedId={null} onOpen={onOpen} {...idle} />,
    );

    // The matched term is wrapped in <mark>, and the markers themselves are gone.
    const mark = screen.getByText("retryBackoff");
    expect(mark.tagName).toBe("MARK");
    expect(screen.getByText("Add retry backoff")).toBeTruthy();
    expect(document.body.textContent).not.toContain(HIGHLIGHT_START);

    fireEvent.click(screen.getByRole("option"));
    expect(onOpen).toHaveBeenCalledWith("s1");

    fireEvent.click(screen.getByText("Add retry backoff"));
    expect(onOpen).toHaveBeenCalledTimes(2);
    expect(onOpen).toHaveBeenLastCalledWith("s1");
  });

  it("shows a Load more control only when more pages exist and fires it on click", () => {
    const onLoadMore = vi.fn();
    const { rerender } = render(
      <SearchResults hits={[hit()]} query="retry" selectedId={null} onOpen={() => {}} {...idle} />,
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
        searching={false}
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
        searching={false}
        hasMore
        loadingMore
        onLoadMore={onLoadMore}
      />,
    );
    expect(screen.getByRole("button", { name: /loading/i }).hasAttribute("disabled")).toBe(true);
  });

  it("exposes a keyboard-operable listbox with a roving active option", () => {
    render(
      <SearchResults
        hits={[hit({ session_id: "a", source_id: "pa", title: "Alpha" }), hit({ session_id: "b", source_id: "pb", title: "Beta" })]}
        query="retry"
        selectedId={null}
        onOpen={() => {}}
        {...idle}
      />,
    );
    // The listbox is focusable and the ARIA listbox pattern is complete.
    const listbox = screen.getByRole("listbox", { name: /search results/i });
    expect(listbox.getAttribute("tabindex")).toBe("0");
    // First option is active by default.
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-0");
    const options = screen.getAllByRole("option");
    expect(options[0].getAttribute("aria-posinset")).toBe("1");
    expect(options[0].getAttribute("aria-setsize")).toBe("2");
    expect(options[0].className).toContain("is-active");

    // ArrowDown/Up move the roving active option (aria-activedescendant tracks it).
    fireEvent.keyDown(listbox, { key: "ArrowDown" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-1");
    expect(screen.getAllByRole("option")[1].className).toContain("is-active");
    fireEvent.keyDown(listbox, { key: "ArrowUp" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-0");

    // j and k also navigate down and up.
    fireEvent.keyDown(listbox, { key: "j" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-1");
    fireEvent.keyDown(listbox, { key: "k" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-0");

    // Home/End jump to the ends; movement is clamped at the boundaries.
    fireEvent.keyDown(listbox, { key: "End" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-1");
    fireEvent.keyDown(listbox, { key: "ArrowDown" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-1");
    fireEvent.keyDown(listbox, { key: "Home" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-0");
  });

  it("calls onExitUp when pressing ArrowUp or k on the first result", () => {
    const onExitUp = vi.fn();
    render(
      <SearchResults
        hits={[hit({ session_id: "a", source_id: "pa" })]}
        query="retry"
        selectedId={null}
        onOpen={() => {}}
        onExitUp={onExitUp}
        {...idle}
      />,
    );
    const listbox = screen.getByRole("listbox", { name: /search results/i });
    fireEvent.keyDown(listbox, { key: "ArrowUp" });
    expect(onExitUp).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(listbox, { key: "k" });
    expect(onExitUp).toHaveBeenCalledTimes(2);
  });

  it("navigates with j, k, Home and End keys", () => {
    render(
      <SearchResults
        hits={[
          hit({ session_id: "a", source_id: "pa" }),
          hit({ session_id: "b", source_id: "pb" }),
          hit({ session_id: "c", source_id: "pc" }),
        ]}
        query="retry"
        selectedId={null}
        onOpen={() => {}}
        {...idle}
      />,
    );
    const listbox = screen.getByRole("listbox", { name: /search results/i });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-0");

    // 'j' moves down
    fireEvent.keyDown(listbox, { key: "j" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-1");

    // 'End' jumps to the end
    fireEvent.keyDown(listbox, { key: "End" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-2");

    // 'k' moves up
    fireEvent.keyDown(listbox, { key: "k" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-1");

    // 'Home' jumps to the start
    fireEvent.keyDown(listbox, { key: "Home" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-0");
  });

  it("opens the active result on Enter", () => {
    const onOpen = vi.fn();
    render(
      <SearchResults
        hits={[hit({ session_id: "a", source_id: "pa" }), hit({ session_id: "b", source_id: "pb" })]}
        query="retry"
        selectedId={null}
        onOpen={onOpen}
        {...idle}
      />,
    );
    const listbox = screen.getByRole("listbox", { name: /search results/i });
    fireEvent.keyDown(listbox, { key: "ArrowDown" });
    fireEvent.keyDown(listbox, { key: "Enter" });
    expect(onOpen).toHaveBeenCalledWith("b");
  });

  it("resets the active option to the top when the query changes", () => {
    const hits = [
      hit({ session_id: "a", source_id: "pa" }),
      hit({ session_id: "b", source_id: "pb" }),
    ];
    const { rerender } = render(
      <SearchResults hits={hits} query="retry" selectedId={null} onOpen={() => {}} {...idle} />,
    );
    const listbox = screen.getByRole("listbox", { name: /search results/i });
    fireEvent.keyDown(listbox, { key: "End" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-1");
    // A new query is a fresh context: the active option returns to the top.
    rerender(
      <SearchResults hits={hits} query="deploy" selectedId={null} onOpen={() => {}} {...idle} />,
    );
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-0");
  });

  it("shows how long ago a result was recorded", () => {
    const { container } = render(
      <SearchResults
        hits={[
          hit({ session_id: "a", source_id: "pa", started_at: Date.now() }),
          hit({ session_id: "b", source_id: "pb", started_at: null }),
        ]}
        query="retry"
        selectedId={null}
        onOpen={() => {}}
        {...idle}
      />,
    );
    // A recent result carries its relative time; a result with no timestamp
    // renders no <time> element (so the row does not mislead about recency).
    expect(screen.getByText("just now")).toBeTruthy();
    expect(container.querySelectorAll("time.results__time").length).toBe(1);
  });

  it("marks the opened session as selected without moving keyboard focus", () => {
    render(
      <SearchResults
        hits={[
          hit({ session_id: "a", source_id: "pa" }),
          hit({ session_id: "b", source_id: "pb" }),
          hit({ session_id: "b", source_id: "pb2" }),
        ]}
        query="retry"
        selectedId="b"
        onOpen={() => {}}
        {...idle}
      />,
    );
    const options = screen.getAllByRole("option");
    // Selection (opened session) is independent of the roving active row.
    expect(options[1].getAttribute("aria-selected")).toBe("true");
    expect(options[0].getAttribute("aria-selected")).toBe("false");
    expect(options[2].getAttribute("aria-selected")).toBe("false");
    expect(options[0].className).toContain("is-active");
  });

  it("renders load more button with aria-busy and handles clicks", () => {
    const onLoadMore = vi.fn();
    const hits = [hit({ session_id: "a" })];
    const { rerender } = render(
      <SearchResults
        hits={hits}
        query="retry"
        selectedId={null}
        onOpen={() => {}}
        {...idle}
        hasMore
        loadingMore={false}
        onLoadMore={onLoadMore}
      />,
    );

    const button = screen.getByRole("button", { name: /load more results/i });
    expect(button.getAttribute("aria-busy")).toBe("false");
    expect(button.hasAttribute("disabled")).toBe(false);

    fireEvent.click(button);
    expect(onLoadMore).toHaveBeenCalledTimes(1);

    rerender(
      <SearchResults
        hits={hits}
        query="retry"
        selectedId={null}
        onOpen={() => {}}
        {...idle}
        hasMore
        loadingMore
        onLoadMore={onLoadMore}
      />,
    );

    const busyButton = screen.getByRole("button", { name: /loading/i });
    expect(busyButton.getAttribute("aria-busy")).toBe("true");
    expect(busyButton.hasAttribute("disabled")).toBe(true);
  });

  it("resets active index to first result when query prop changes", () => {
    const hitsA = [
      hit({ session_id: "a1", source_id: "p1", title: "First A" }),
      hit({ session_id: "a2", source_id: "p2", title: "Second A" }),
    ];
    const { rerender } = render(
      <SearchResults
        hits={hitsA}
        query="queryA"
        selectedId={null}
        onOpen={() => {}}
        {...idle}
      />,
    );
    const listbox = screen.getByRole("listbox", { name: /search results/i });

    // Navigate to second result
    fireEvent.keyDown(listbox, { key: "ArrowDown" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-1");

    // Change query prop
    const hitsB = [
      hit({ session_id: "b1", source_id: "pb1", title: "First B" }),
      hit({ session_id: "b2", source_id: "pb2", title: "Second B" }),
    ];
    rerender(
      <SearchResults
        hits={hitsB}
        query="queryB"
        selectedId={null}
        onOpen={() => {}}
        {...idle}
      />,
    );
    const updatedListbox = screen.getByRole("listbox", { name: /search results/i });
    expect(updatedListbox.getAttribute("aria-activedescendant")).toBe("search-result-0");
  });

  it("navigates to first and last items using Home and End keys", () => {
    const hits = [
      hit({ session_id: "s1", source_id: "p1", title: "First" }),
      hit({ session_id: "s2", source_id: "p2", title: "Second" }),
      hit({ session_id: "s3", source_id: "p3", title: "Third" }),
    ];
    const onOpen = vi.fn();
    render(
      <SearchResults
        hits={hits}
        query="test"
        selectedId={null}
        onOpen={onOpen}
        {...idle}
      />,
    );
    const listbox = screen.getByRole("listbox", { name: /search results/i });

    fireEvent.keyDown(listbox, { key: "End" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-2");

    fireEvent.keyDown(listbox, { key: "Enter" });
    expect(onOpen).toHaveBeenCalledWith("s3");

    fireEvent.keyDown(listbox, { key: "Home" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-0");

    fireEvent.keyDown(listbox, { key: "Enter" });
    expect(onOpen).toHaveBeenCalledWith("s1");
  });

  it("handles multiple highlighted segments within the same snippet", () => {
    const multiHit = hit({
      snippet: `check ${HIGHLIGHT_START}foo${HIGHLIGHT_END} and ${HIGHLIGHT_START}bar${HIGHLIGHT_END} functions`,
    });
    render(
      <SearchResults
        hits={[multiHit]}
        query="foo bar"
        selectedId={null}
        onOpen={() => {}}
        {...idle}
      />,
    );
    const marks = screen.getAllByRole("mark", { hidden: true });
    expect(marks.length).toBe(2);
    expect(marks[0].textContent).toBe("foo");
    expect(marks[1].textContent).toBe("bar");
  });

  it("navigates with j and k keys and sets accessibility attributes", () => {
    const hits = [
      hit({ session_id: "s1", source_id: "p1", title: "First" }),
      hit({ session_id: "s2", source_id: "p2", title: "Second" }),
    ];
    render(
      <SearchResults
        hits={hits}
        query="test"
        selectedId={null}
        onOpen={() => {}}
        {...idle}
      />,
    );
    const listbox = screen.getByRole("listbox", { name: /search results/i });
    const options = screen.getAllByRole("option");
    expect(options[0].getAttribute("aria-setsize")).toBe("2");
    expect(options[0].getAttribute("aria-posinset")).toBe("1");

    fireEvent.keyDown(listbox, { key: "j" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-1");

    fireEvent.keyDown(listbox, { key: "k" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("search-result-0");
  });
});
