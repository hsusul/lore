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
});
