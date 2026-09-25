import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import CommandPalette, { filterItems, fuzzyScore, type PaletteItem } from "./CommandPalette";

function items(): PaletteItem[] {
  return [
    { id: "a", group: "Command", label: "Open folder…", run: vi.fn() },
    { id: "b", group: "Command", label: "Toggle Panel", run: vi.fn() },
    { id: "c", group: "File", label: "main.ts", detail: "src/main.ts · repo", run: vi.fn() },
    { id: "d", group: "Agent", label: "Fix parser", detail: "Running · Claude Code · lore/fix-parser", run: vi.fn() },
  ];
}

describe("fuzzyScore", () => {
  it("matches ordered subsequences and ranks tighter matches higher", () => {
    expect(fuzzyScore("tgp", "Toggle Panel")).not.toBeNull();
    expect(fuzzyScore("pt", "Toggle Panel")).toBeNull();
    expect(fuzzyScore("panel", "Toggle Panel")!).toBeGreaterThan(fuzzyScore("pnl", "Toggle Panel")!);
  });

  it("filters by label or detail", () => {
    expect(filterItems(items(), "src/m").map((i) => i.id)).toEqual(["c"]);
    // With no query: commands, then agents, then files.
    expect(filterItems(items(), "").map((i) => i.id)).toEqual(["a", "b", "d", "c"]);
    expect(filterItems(items(), "parser").map((i) => i.id)).toEqual(["d"]);
  });
});

describe("CommandPalette", () => {
  it("filters, runs the highlighted item on Enter, and closes", () => {
    const list = items();
    const onClose = vi.fn();
    render(<CommandPalette items={list} onClose={onClose} />);
    const input = screen.getByRole("combobox");
    expect(document.activeElement).toBe(input);

    fireEvent.change(input, { target: { value: "tog" } });
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["Toggle Panel"]);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onClose).toHaveBeenCalled();
    expect(list[1].run).toHaveBeenCalled();
  });

  it("moves with arrows, closes on Escape, and returns focus", () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const list = items();
    const onClose = vi.fn();
    const { unmount } = render(<CommandPalette items={list} onClose={onClose} />);
    const input = screen.getByRole("combobox");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getAllByRole("option")[1].getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(input, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
    expect(list[1].run).not.toHaveBeenCalled();
    unmount();
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  it("offers a new agent from whatever was typed, after the matches", () => {
    const onNewAgent = vi.fn();
    const onClose = vi.fn();
    render(<CommandPalette items={items()} onClose={onClose} onNewAgent={onNewAgent} />);
    const input = screen.getByRole("combobox");
    fireEvent.change(input, { target: { value: "add dark mode" } });
    const options = screen.getAllByRole("option");
    expect(options[options.length - 1].textContent).toContain("New agent: add dark mode");
    fireEvent.keyDown(input, { key: "End" });
    fireEvent.click(options[options.length - 1]);
    expect(onClose).toHaveBeenCalled();
    expect(onNewAgent).toHaveBeenCalledWith("add dark mode");
  });

  it("keeps focus in the search field on Tab", () => {
    render(<CommandPalette items={items()} onClose={vi.fn()} />);
    const input = screen.getByRole("combobox");
    const tab = fireEvent.keyDown(input, { key: "Tab" });
    expect(tab).toBe(false);
    expect(document.activeElement).toBe(input);
  });
});
