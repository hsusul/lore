import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

// SettingsPanel renders BackupSettings, which reads the schedule on mount.
vi.mock("../ipc", () => ({
  getBackupSchedule: vi.fn().mockResolvedValue({ interval: "off", keep: 7 }),
  setBackupSchedule: vi.fn().mockResolvedValue(undefined),
  backupNow: vi.fn().mockResolvedValue(undefined),
}));

import SettingsPanel from "./SettingsPanel";
import type { DetectedAgent } from "../ipc";

const agents: DetectedAgent[] = [
  {
    id: "codex",
    display_name: "Codex",
    installed: true,
    version: null,
    session_count: 12,
    roots: ["/Users/dev/.codex/sessions", "/Volumes/archive/codex"],
    custom_roots: ["/Volumes/archive/codex"],
  },
];

const baseProps = {
  agents,
  rootBusy: null,
  onAddAgentRoot: vi.fn(),
  onRemoveAgentRoot: vi.fn(),
  onForgetEverything: vi.fn(),
  onClose: vi.fn(),
};

describe("SettingsPanel", () => {
  it("renders nothing when closed", () => {
    const { container } = render(
      <SettingsPanel
        open={false}
        {...baseProps}
      />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("states the honest privacy posture and lists agents", () => {
    render(
      <SettingsPanel
        open
        {...baseProps}
      />,
    );
    expect(screen.getByRole("dialog", { name: /settings/i })).toBeTruthy();
    expect(screen.getByText(/no network connection by default/i)).toBeTruthy();
    expect(screen.getByText(/not a guarantee/i)).toBeTruthy();
    expect(screen.getByText(/not encryption/i)).toBeTruthy();
    expect(screen.getByText("Codex")).toBeTruthy();
    expect(screen.getByText("/Users/dev/.codex/sessions")).toBeTruthy();
    expect(screen.getByText("/Volumes/archive/codex")).toBeTruthy();
    expect(screen.getByText("detected")).toBeTruthy();
  });

  it("adds and removes custom agent folders without hiding the default root", () => {
    const onAddAgentRoot = vi.fn();
    const onRemoveAgentRoot = vi.fn();
    render(
      <SettingsPanel
        open
        {...baseProps}
        onAddAgentRoot={onAddAgentRoot}
        onRemoveAgentRoot={onRemoveAgentRoot}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Add another Codex folder" }));
    expect(onAddAgentRoot).toHaveBeenCalledWith("codex", "Codex");

    fireEvent.click(
      screen.getByRole("button", { name: "Remove /Volumes/archive/codex from Codex" }),
    );
    expect(onRemoveAgentRoot).toHaveBeenCalledWith("codex", "/Volumes/archive/codex");
  });

  it("disables folder mutations while that agent is updating", () => {
    render(
      <SettingsPanel
        open
        {...baseProps}
        rootBusy="codex"
      />,
    );

    expect(
      (screen.getByRole("button", { name: "Updating folders…" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(
      (
        screen.getByRole("button", {
          name: "Remove /Volumes/archive/codex from Codex",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
  });

  it("triggers forget-everything and closes on Escape", () => {
    const onForget = vi.fn();
    const onClose = vi.fn();
    render(
      <SettingsPanel
        open
        {...baseProps}
        onForgetEverything={onForget}
        onClose={onClose}
      />,
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

    const { rerender } = render(<SettingsPanel open={false} {...baseProps} />);

    // Opening moves focus into the dialog…
    rerender(<SettingsPanel open {...baseProps} />);
    const dialog = screen.getByRole("dialog");
    expect(dialog.contains(document.activeElement)).toBe(true);

    // …and closing returns it to the element that had focus before.
    rerender(<SettingsPanel open={false} {...baseProps} />);
    expect(document.activeElement).toBe(trigger);

    trigger.remove();
  });

  it("traps Tab focus inside the dialog", () => {
    render(
      <SettingsPanel
        open
        {...baseProps}
      />,
    );
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

  it("closes when clicking the close button or backdrop", () => {
    const onClose = vi.fn();
    const { container } = render(
      <SettingsPanel
        open
        {...baseProps}
        onClose={onClose}
      />,
    );

    // Click close icon button
    fireEvent.click(screen.getByRole("button", { name: /close settings/i }));
    expect(onClose).toHaveBeenCalledTimes(1);

    // Click backdrop
    const backdrop = container.querySelector(".modal-backdrop");
    expect(backdrop).toBeTruthy();
    if (backdrop) fireEvent.click(backdrop);
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});
