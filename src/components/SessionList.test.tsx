import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import SessionList from "./SessionList";
import type { SessionSummary } from "../ipc";

function summary(id: string, title: string, parse_status = "ok"): SessionSummary {
  return {
    id,
    agent_id: "codex",
    title,
    started_at: null,
    ended_at: null,
    message_count: 1,
    tool_call_count: 0,
    primary_model: null,
    parse_status,
  };
}

const sessions = [
  summary("a", "first"),
  summary("b", "second", "partial"),
  summary("c", "third"),
];

describe("SessionList", () => {
  it("shows an empty state with no sessions", () => {
    render(<SessionList sessions={[]} selectedId={null} onOpen={() => {}} />);
    expect(screen.getByText(/no sessions/i)).toBeTruthy();
  });

  it("renders a partial-parse badge", () => {
    render(<SessionList sessions={sessions} selectedId={null} onOpen={() => {}} />);
    expect(screen.getByText("partial")).toBeTruthy();
  });

  it("opens the active row on Enter and navigates with arrows", () => {
    const onOpen = vi.fn();
    render(<SessionList sessions={sessions} selectedId={null} onOpen={onOpen} />);
    const listbox = screen.getByRole("listbox", { name: /sessions/i });

    // Arrow Down twice → third row active, Enter opens it.
    fireEvent.keyDown(listbox, { key: "ArrowDown" });
    fireEvent.keyDown(listbox, { key: "ArrowDown" });
    fireEvent.keyDown(listbox, { key: "Enter" });
    expect(onOpen).toHaveBeenCalledWith("c");

    // Arrow Up back to the second row, Enter opens it.
    fireEvent.keyDown(listbox, { key: "ArrowUp" });
    fireEvent.keyDown(listbox, { key: "Enter" });
    expect(onOpen).toHaveBeenLastCalledWith("b");
  });

  it("navigates with j/k and End keys", () => {
    const onOpen = vi.fn();
    render(<SessionList sessions={sessions} selectedId={null} onOpen={onOpen} />);
    const listbox = screen.getByRole("listbox", { name: /sessions/i });

    fireEvent.keyDown(listbox, { key: "j" }); // → second
    fireEvent.keyDown(listbox, { key: "Enter" });
    expect(onOpen).toHaveBeenLastCalledWith("b");

    fireEvent.keyDown(listbox, { key: "k" }); // → first
    fireEvent.keyDown(listbox, { key: "Enter" });
    expect(onOpen).toHaveBeenLastCalledWith("a");

    fireEvent.keyDown(listbox, { key: "End" }); // → last
    fireEvent.keyDown(listbox, { key: "Enter" });
    expect(onOpen).toHaveBeenLastCalledWith("c");
  });

  it("opens a row on click", () => {
    const onOpen = vi.fn();
    render(<SessionList sessions={sessions} selectedId={null} onOpen={onOpen} />);
    fireEvent.click(screen.getByText("first"));
    expect(onOpen).toHaveBeenCalledWith("a");
  });

  it("loads older sessions without putting a button inside the listbox", () => {
    const onLoadMore = vi.fn();
    render(
      <SessionList
        sessions={sessions}
        selectedId={null}
        onOpen={() => {}}
        hasMore
        loadingMore={false}
        onLoadMore={onLoadMore}
      />,
    );

    const listbox = screen.getByRole("listbox", { name: /sessions/i });
    const button = screen.getByRole("button", { name: /load older sessions/i });
    expect(listbox.contains(button)).toBe(false);
    fireEvent.click(button);
    expect(onLoadMore).toHaveBeenCalledTimes(1);
  });

  it("navigates to the first item with Home key and resets index on session list change", () => {
    const onOpen = vi.fn();
    const { rerender } = render(
      <SessionList sessions={sessions} selectedId={null} onOpen={onOpen} />,
    );
    const listbox = screen.getByRole("listbox", { name: /sessions/i });

    // Jump to last, then jump back to start with Home
    fireEvent.keyDown(listbox, { key: "End" });
    fireEvent.keyDown(listbox, { key: "Home" });
    fireEvent.keyDown(listbox, { key: "Enter" });
    expect(onOpen).toHaveBeenLastCalledWith("a");

    // When sessions list changes to another set, navigation resets to index 0
    const newSessions = [summary("x", "new first"), summary("y", "new second")];
    rerender(<SessionList sessions={newSessions} selectedId={null} onOpen={onOpen} />);
    const updatedListbox = screen.getByRole("listbox", { name: /sessions/i });
    fireEvent.keyDown(updatedListbox, { key: "Enter" });
    expect(onOpen).toHaveBeenLastCalledWith("x");
  });
});
