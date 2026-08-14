import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import SettingsPanel from "./SettingsPanel";
import type { DetectedAgent } from "../ipc";

const agents: DetectedAgent[] = [
  { id: "codex", display_name: "Codex", installed: true, version: null, session_count: 12 },
];

describe("SettingsPanel", () => {
  it("renders nothing when closed", () => {
    const { container } = render(
      <SettingsPanel open={false} agents={agents} onForgetEverything={() => {}} onClose={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("states the honest privacy posture and lists agents", () => {
    render(
      <SettingsPanel open agents={agents} onForgetEverything={() => {}} onClose={() => {}} />,
    );
    expect(screen.getByRole("dialog", { name: /settings/i })).toBeTruthy();
    expect(screen.getByText(/no network connection by default/i)).toBeTruthy();
    expect(screen.getByText(/not a guarantee/i)).toBeTruthy();
    expect(screen.getByText(/not encryption/i)).toBeTruthy();
    expect(screen.getByText("Codex")).toBeTruthy();
  });

  it("triggers forget-everything and closes on Escape", () => {
    const onForget = vi.fn();
    const onClose = vi.fn();
    render(
      <SettingsPanel open agents={agents} onForgetEverything={onForget} onClose={onClose} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /forget everything/i }));
    expect(onForget).toHaveBeenCalled();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });

  it("moves focus into the dialog on open and restores it on close", () => {
    const trigger = document.createElement("button");
    document.body.appendChild(trigger);
    trigger.focus();
    expect(document.activeElement).toBe(trigger);

    const props = { agents, onForgetEverything: () => {}, onClose: () => {} };
    const { rerender } = render(<SettingsPanel open={false} {...props} />);

    // Opening moves focus into the dialog…
    rerender(<SettingsPanel open {...props} />);
    const dialog = screen.getByRole("dialog");
    expect(dialog.contains(document.activeElement)).toBe(true);

    // …and closing returns it to the element that had focus before.
    rerender(<SettingsPanel open={false} {...props} />);
    expect(document.activeElement).toBe(trigger);

    trigger.remove();
  });

  it("traps Tab focus inside the dialog", () => {
    render(<SettingsPanel open agents={agents} onForgetEverything={() => {}} onClose={() => {}} />);
    const dialog = screen.getByRole("dialog");
    const buttons = screen.getAllByRole("button");
    const first = buttons[0];
    const last = buttons[buttons.length - 1];

    // Tab off the last focusable wraps to the first…
    last.focus();
    fireEvent.keyDown(dialog, { key: "Tab" });
    expect(document.activeElement).toBe(first);

    // …and Shift+Tab off the first wraps to the last.
    first.focus();
    fireEvent.keyDown(dialog, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(last);
  });
});
