import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import CommandPalette, { type Command } from "./CommandPalette";

function makeItems(): Command[] {
  return [
    { id: "a", group: "Repository", label: "billing-service", hint: "12 sessions", run: vi.fn() },
    { id: "b", group: "Session", label: "Add webhook", hint: "Codex", run: vi.fn() },
    { id: "c", group: "Session", label: "Refactor retry", hint: "Claude", run: vi.fn() },
  ];
}

describe("CommandPalette", () => {
  it("renders nothing when closed", () => {
    const { container } = render(
      <CommandPalette items={makeItems()} open={false} onClose={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("filters as you type and runs the top match on Enter", () => {
    const items = makeItems();
    const onClose = vi.fn();
    render(<CommandPalette items={items} open onClose={onClose} />);
    const input = screen.getByRole("combobox");

    fireEvent.change(input, { target: { value: "refactor" } });
    expect(screen.queryByText("billing-service")).toBeNull();
    expect(screen.getByText("Refactor retry")).toBeTruthy();

    fireEvent.keyDown(input, { key: "Enter" });
    expect(items[2].run).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("navigates with arrow keys", () => {
    const items = makeItems();
    render(<CommandPalette items={items} open onClose={() => {}} />);
    const input = screen.getByRole("combobox");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(items[1].run).toHaveBeenCalledTimes(1);
  });

  it("closes on Escape", () => {
    const onClose = vi.fn();
    render(<CommandPalette items={makeItems()} open onClose={onClose} />);
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("shows an empty state when nothing matches", () => {
    render(<CommandPalette items={makeItems()} open onClose={() => {}} />);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "zzzz" } });
    expect(screen.getByText(/no matches/i)).toBeTruthy();
  });
});
