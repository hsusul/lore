import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import CommandPalette, { type Command } from "./CommandPalette";

function makeItems(): Command[] {
  return [
    { id: "a", group: "Repository", label: "billing-service", hint: "12 sessions", run: vi.fn() },
    { id: "b", group: "Session", label: "Add webhook", hint: "Codex", run: vi.fn() },
    { id: "c", group: "Session", label: "Refactor retry", hint: "Claude", run: vi.fn() },
  ];
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("CommandPalette", () => {
  it("focuses its search field when mounted", () => {
    render(<CommandPalette items={makeItems()} onClose={() => {}} />);
    expect(document.activeElement).toBe(
      screen.getByRole("combobox", { name: /search commands and sessions/i }),
    );
  });

  it("filters as you type and runs the top match on Enter", () => {
    const items = makeItems();
    const onClose = vi.fn();
    render(<CommandPalette items={items} onClose={onClose} />);
    const input = screen.getByRole("combobox");

    fireEvent.change(input, { target: { value: "refactor" } });
    expect(screen.queryByText("billing-service")).toBeNull();
    expect(screen.getByText("Refactor retry")).toBeTruthy();

    fireEvent.keyDown(input, { key: "Enter" });
    expect(items[2].run).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("finds loaded commands with an ordered fuzzy match", () => {
    const items = makeItems();
    render(<CommandPalette items={items} onClose={() => {}} />);
    const input = screen.getByRole("combobox");

    fireEvent.change(input, { target: { value: "rfrr" } });

    expect(screen.queryByText("billing-service")).toBeNull();
    expect(screen.getByText("Refactor retry")).toBeTruthy();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(items[2].run).toHaveBeenCalledTimes(1);
  });

  it("navigates with arrow keys", () => {
    const items = makeItems();
    render(<CommandPalette items={items} onClose={() => {}} />);
    const input = screen.getByRole("combobox");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(items[1].run).toHaveBeenCalledTimes(1);
  });

  it("jumps to the first and last match with Home and End", () => {
    const items = makeItems();
    render(<CommandPalette items={items} onClose={() => {}} />);
    const input = screen.getByRole("combobox");

    fireEvent.keyDown(input, { key: "End" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(items[2].run).toHaveBeenCalledTimes(1);
  });

  it("closes on Escape", () => {
    const onClose = vi.fn();
    render(<CommandPalette items={makeItems()} onClose={onClose} />);
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("shows an empty state when nothing matches", () => {
    render(<CommandPalette items={makeItems()} onClose={() => {}} />);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "zzzz" } });
    expect(screen.getByText(/no matches/i)).toBeTruthy();
  });

  it("debounces archive search and runs a returned session", async () => {
    const run = vi.fn();
    const search = vi.fn().mockResolvedValue([
      {
        id: "remote-session",
        group: "Archive",
        // The archive may match message content even when the title does not.
        label: "Investigate checkout outage",
        hint: "Codex · archive match",
        run,
      },
    ] satisfies Command[]);
    vi.useFakeTimers();

    try {
      render(<CommandPalette items={makeItems()} search={search} onClose={() => {}} />);
      const input = screen.getByRole("combobox");
      fireEvent.change(input, { target: { value: "backoff" } });

      expect(search).not.toHaveBeenCalled();
      expect(screen.getByRole("status").textContent).toContain("Searching archive");

      await act(async () => {
        await vi.advanceTimersByTimeAsync(200);
      });

      expect(search).toHaveBeenCalledWith("backoff");
      expect(screen.getByText("Investigate checkout outage")).toBeTruthy();
      fireEvent.keyDown(input, { key: "Enter" });
      expect(run).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("ignores archive results from a superseded query", async () => {
    const alpha = deferred<Command[]>();
    const beta = deferred<Command[]>();
    const search = vi.fn((query: string) => (query === "alpha" ? alpha.promise : beta.promise));
    vi.useFakeTimers();

    try {
      render(<CommandPalette items={[]} search={search} onClose={() => {}} />);
      const input = screen.getByRole("combobox");

      fireEvent.change(input, { target: { value: "alpha" } });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(200);
      });
      fireEvent.change(input, { target: { value: "beta" } });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(200);
      });

      await act(async () => {
        beta.resolve([
          { id: "beta", label: "Beta result", group: "Archive", run: vi.fn() },
        ]);
      });
      expect(screen.getByText("Beta result")).toBeTruthy();

      await act(async () => {
        alpha.resolve([
          { id: "alpha", label: "Alpha result", group: "Archive", run: vi.fn() },
        ]);
      });
      expect(screen.queryByText("Alpha result")).toBeNull();
      expect(screen.getByText("Beta result")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });
});
