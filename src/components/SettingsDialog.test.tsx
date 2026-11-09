import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { AutoLessonsDto } from "../ipc";

const events = vi.hoisted(() => ({ auto: null as null | ((s: AutoLessonsDto) => void) }));

vi.mock("../ipc", () => ({
  backupNow: vi.fn(),
  getBackupSchedule: vi.fn(),
  setBackupSchedule: vi.fn(),
  getAutoLessons: vi.fn(),
  setAutoLessons: vi.fn(),
  onAutoLessons: vi.fn(async (handler: (s: AutoLessonsDto) => void) => {
    events.auto = handler;
    return () => {};
  }),
}));

import * as ipc from "../ipc";
import { AutoLessons } from "./SettingsDialog";

const m = vi.mocked(ipc);

const state = (over: Partial<AutoLessonsDto> = {}): AutoLessonsDto => ({
  enabled: true,
  running: false,
  last_run_ms: null,
  last_check_ms: null,
  last_outcome: null,
  next_ms: null,
  ...over,
});

describe("Find lessons automatically", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    events.auto = null;
  });

  it("reads the setting and writes it when toggled", async () => {
    m.getAutoLessons.mockResolvedValue(state());
    m.setAutoLessons.mockResolvedValue(state({ enabled: false }));
    render(<AutoLessons />);
    const toggle = await screen.findByRole("checkbox", { name: "Find lessons automatically" });
    expect((toggle as HTMLInputElement).checked).toBe(true);
    expect(screen.getByText(/at most once a day.*Sonnet for Claude Code.*your agent's usage/i)).toBeTruthy();
    expect(screen.getByRole("status").textContent).toBe("No automatic run yet.");

    fireEvent.click(toggle);
    expect(m.setAutoLessons).toHaveBeenCalledWith(false);
    await vi.waitFor(() => expect((toggle as HTMLInputElement).checked).toBe(false));
  });

  it("shows the last automatic run and its outcome, and follows the run as it goes", async () => {
    const at = Date.UTC(2026, 10, 4, 9, 30);
    m.getAutoLessons.mockResolvedValue(
      state({ last_run_ms: at, last_check_ms: at, last_outcome: "Claude Code read 3 new corrections and filed 1 lesson." }),
    );
    render(<AutoLessons />);
    const line = await screen.findByText(/^Last automatic run, .+: Claude Code read 3 new corrections and filed 1 lesson\.$/);
    expect(line.getAttribute("role")).toBe("status");

    await vi.waitFor(() => expect(events.auto).not.toBeNull());
    act(() => events.auto?.(state({ running: true, last_run_ms: at + 1 })));
    expect(screen.getByRole("status").textContent).toBe("Finding lessons now…");
    act(() => events.auto?.(state({ last_check_ms: at + 2, last_outcome: "No new corrections." })));
    expect(screen.getByRole("status").textContent).toMatch(/: No new corrections\.$/);
  });

  it("says when a run that did not finish is tried again", async () => {
    const at = Date.UTC(2026, 10, 8, 18, 36);
    const failed = state({
      last_run_ms: at,
      last_check_ms: at,
      last_outcome: "Claude Code failed: timed out",
      next_ms: at + 60 * 60 * 1000,
    });
    m.getAutoLessons.mockResolvedValue(failed);
    render(<AutoLessons />);
    const line = await screen.findByText(/^Last automatic run, .+: Claude Code failed: timed out Next try .+\.$/);
    expect(line.getAttribute("role")).toBe("status");

    await vi.waitFor(() => expect(events.auto).not.toBeNull());
    act(() => events.auto?.({ ...failed, enabled: false }));
    expect(screen.getByRole("status").textContent).toMatch(/: Claude Code failed: timed out$/);
  });

  it("says when the setting cannot be saved", async () => {
    m.getAutoLessons.mockResolvedValue(state());
    m.setAutoLessons.mockRejectedValue("database is locked");
    render(<AutoLessons />);
    fireEvent.click(await screen.findByRole("checkbox", { name: "Find lessons automatically" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Could not save the setting: database is locked");
  });
});
