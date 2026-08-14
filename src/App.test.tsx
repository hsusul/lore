import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => ({
  listDetectedAgents: vi.fn().mockResolvedValue([]),
  listRepositories: vi.fn().mockResolvedValue([]),
  listSessions: vi.fn().mockResolvedValue([]),
  listRepositorySessions: vi.fn().mockResolvedValue([]),
  getSession: vi.fn().mockResolvedValue(null),
  getGitSnapshot: vi.fn().mockResolvedValue([]),
  getFilePatch: vi.fn().mockResolvedValue(null),
  sessionSecretCount: vi.fn().mockResolvedValue(0),
  exportSessionMarkdown: vi.fn().mockResolvedValue(""),
  forgetSession: vi.fn().mockResolvedValue({ blobs_removed: 0, source_paths: [] }),
  forgetEverything: vi.fn().mockResolvedValue({ blobs_removed: 0, source_paths: [] }),
  search: vi.fn().mockResolvedValue([]),
  searchPage: vi.fn().mockResolvedValue({ hits: [], next_cursor: null }),
  rescan: vi.fn().mockResolvedValue({}),
  onScanProgress: vi.fn().mockResolvedValue(() => {}),
  HIGHLIGHT_START: "\u{e000}",
  HIGHLIGHT_END: "\u{e001}",
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

  it("opens the command palette on ⌘K", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Lore", level: 1 });
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(screen.getByRole("dialog", { name: /command palette/i })).toBeTruthy();
  });
});
