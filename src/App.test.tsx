import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => ({
  listDetectedAgents: vi.fn().mockResolvedValue([]),
  listSessions: vi.fn().mockResolvedValue([]),
  rescan: vi.fn().mockResolvedValue({}),
  onScanProgress: vi.fn().mockResolvedValue(() => {}),
}));

import App from "./App";

describe("App shell", () => {
  it("renders the heading and a rescan control", async () => {
    render(<App />);
    // findBy* flushes the initial data-loading effect inside act().
    expect(
      await screen.findByRole("heading", { name: "Lore", level: 1 }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: /rescan/i })).toBeTruthy();
  });
});
