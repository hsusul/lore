import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import CommandPalette, { filterItems, fuzzyScore, type PaletteItem } from "./CommandPalette";

function items(): PaletteItem[] {
  return [
    { id: "a", group: "Command", label: "Open Folder…", run: vi.fn() },
    { id: "b", group: "Command", label: "Toggle Panel", run: vi.fn() },
    { id: "c", group: "File", label: "main.ts", detail: "src/main.ts · repo", run: vi.fn() },
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
    expect(filterItems(items(), "").map((i) => i.id)).toEqual(["a", "b", "c"]);
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
});
