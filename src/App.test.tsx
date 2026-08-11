import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => ({
  listDetectedAgents: vi.fn().mockResolvedValue([]),
  listRepositories: vi.fn().mockResolvedValue([]),
  listSessions: vi.fn().mockResolvedValue([]),
  listRepositorySessions: vi.fn().mockResolvedValue([]),
  getSession: vi.fn().mockResolvedValue(null),
  getGitSnapshot: vi.fn().mockResolvedValue([]),
  rescan: vi.fn().mockResolvedValue({}),
  onScanProgress: vi.fn().mockResolvedValue(() => {}),
}));

import App from "./App";

describe("App shell", () => {
  it("renders the three-pane shell heading and a rescan control", async () => {
    render(<App />);
    expect(
      await screen.findByRole("heading", { name: "Lore", level: 1 }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: /rescan/i })).toBeTruthy();
    // Panes present: repositories nav and sessions listbox.
    expect(screen.getByRole("navigation", { name: /repositories/i })).toBeTruthy();
  });
});
