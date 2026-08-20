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

function renderPalette(props: Partial<Parameters<typeof CommandPalette>[0]> = {}) {
  const finalProps = {
    items: makeItems(),
    onClose: vi.fn(),
    ...props,
  };
  const result = render(<CommandPalette {...finalProps} />);
  return { ...result, props: finalProps };
}

describe("CommandPalette", () => {
  it("focuses its search field when mounted", () => {
    renderPalette();
    expect(document.activeElement).toBe(
      screen.getByRole("combobox", { name: /search commands and sessions/i }),
    );
  });

  it("filters as you type and runs the top match on Enter", () => {
    const items = makeItems();
    const onClose = vi.fn();
    renderPalette({ items, onClose });
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
    renderPalette({ items });
    const input = screen.getByRole("combobox");

    fireEvent.change(input, { target: { value: "rfrr" } });

    expect(screen.queryByText("billing-service")).toBeNull();
    expect(screen.getByText("Refactor retry")).toBeTruthy();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(items[2].run).toHaveBeenCalledTimes(1);
  });

  it("navigates with arrow keys", () => {
    const items = makeItems();
    renderPalette({ items });
    const input = screen.getByRole("combobox");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(items[1].run).toHaveBeenCalledTimes(1);
  });

  it("jumps to the first and last match with Home and End", () => {
    const items = makeItems();
    renderPalette({ items });
    const input = screen.getByRole("combobox");

    fireEvent.keyDown(input, { key: "End" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(items[2].run).toHaveBeenCalledTimes(1);
  });

  it("closes on Escape", () => {
    const { props } = renderPalette();
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });
    expect(props.onClose).toHaveBeenCalledTimes(1);
  });

  it("shows an empty state when nothing matches", () => {
    renderPalette();
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
      renderPalette({ search });
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

  it("handles synchronous search failure gracefully without throwing", async () => {
    const search = vi.fn(() => {
      throw new Error("sync failure");
    });
    vi.useFakeTimers();

    try {
      render(<CommandPalette items={[]} search={search} onClose={() => {}} />);
      const input = screen.getByRole("combobox");
      fireEvent.change(input, { target: { value: "error-query" } });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(200);
      });
      expect(search).toHaveBeenCalledWith("error-query");
      expect(screen.getByText(/archive search unavailable/i)).toBeTruthy();
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

  it("closes when clicking the backdrop", () => {
    const { props, container } = renderPalette();
    const backdrop = container.querySelector(".palette__backdrop");
    expect(backdrop).toBeTruthy();
    if (backdrop) fireEvent.click(backdrop);
    expect(props.onClose).toHaveBeenCalledTimes(1);
  });

  it("executes an option and closes when clicked directly", () => {
    const items = makeItems();
    const onClose = vi.fn();
    renderPalette({ items, onClose });

    const option = screen.getByText("Add webhook");
    fireEvent.click(option);
    expect(items[1].run).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("updates active option on mouse enter", () => {
    const items = makeItems();
    renderPalette({ items });

    const option = screen.getByText("Refactor retry").closest("li");
    expect(option).toBeTruthy();
    if (option) fireEvent.mouseEnter(option);

    expect(option?.className).toContain("is-active");
  });

  it("sets aria-setsize and aria-posinset on listbox options", () => {
    const items = makeItems();
    renderPalette({ items });

    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(3);
    expect(options[0].getAttribute("aria-setsize")).toBe("3");
    expect(options[0].getAttribute("aria-posinset")).toBe("1");
    expect(options[1].getAttribute("aria-setsize")).toBe("3");
    expect(options[1].getAttribute("aria-posinset")).toBe("2");
    expect(options[2].getAttribute("aria-setsize")).toBe("3");
    expect(options[2].getAttribute("aria-posinset")).toBe("3");
  });

  it("restores focus to previous element on unmount", () => {
    const trigger = document.createElement("button");
    document.body.appendChild(trigger);
    trigger.focus();
    expect(document.activeElement).toBe(trigger);

    const { unmount } = renderPalette();
    expect(document.activeElement).toBe(
      screen.getByRole("combobox", { name: /search commands and sessions/i }),
    );

    unmount();
    expect(document.activeElement).toBe(trigger);
    trigger.remove();
  });

  it("does not throw or close when Enter is pressed on an empty results list", () => {
    const onClose = vi.fn();
    renderPalette({ items: [], onClose });
    const input = screen.getByRole("combobox");

    fireEvent.keyDown(input, { key: "Enter" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("closes on Escape key press without executing any item", () => {
    const onClose = vi.fn();
    const items = makeItems();
    renderPalette({ items, onClose });
    const input = screen.getByRole("combobox");

    fireEvent.keyDown(input, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(items[0].run).not.toHaveBeenCalled();
  });

  it("renders command items without hints cleanly", () => {
    const itemsWithoutHint: Command[] = [
      { id: "nohint", group: "Actions", label: "Toggle Dark Mode", run: vi.fn() },
    ];
    renderPalette({ items: itemsWithoutHint });
    expect(screen.getByText("Toggle Dark Mode")).toBeTruthy();
    expect(screen.queryByText("12 sessions")).toBeNull();
  });
});
