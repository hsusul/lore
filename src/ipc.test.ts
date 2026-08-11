import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const listen = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: (...a: unknown[]) => listen(...a) }));

import { listDetectedAgents, listSessions, onScanProgress, rescan } from "./ipc";

beforeEach(() => {
  invoke.mockReset();
  listen.mockReset();
});

describe("ipc contract", () => {
  it("list_detected_agents invokes its command", async () => {
    invoke.mockResolvedValue([]);
    await listDetectedAgents();
    expect(invoke).toHaveBeenCalledWith("list_detected_agents");
  });

  it("list_sessions passes the limit argument", async () => {
    invoke.mockResolvedValue([]);
    await listSessions(50);
    expect(invoke).toHaveBeenCalledWith("list_sessions", { limit: 50 });
  });

  it("rescan invokes its command and returns the tally", async () => {
    invoke.mockResolvedValue({
      discovered: 3,
      ingested: 2,
      skipped: 1,
      failed: 0,
      enriched: 2,
    });
    const result = await rescan();
    expect(invoke).toHaveBeenCalledWith("rescan");
    expect(result.ingested).toBe(2);
  });

  it("onScanProgress subscribes to the event and forwards payloads", async () => {
    let handler: ((event: { payload: unknown }) => void) | undefined;
    listen.mockImplementation(
      (_name: string, cb: (event: { payload: unknown }) => void) => {
        handler = cb;
        return Promise.resolve(() => {});
      },
    );
    const seen: unknown[] = [];
    await onScanProgress((p) => seen.push(p));
    expect(listen).toHaveBeenCalledWith("scan_progress", expect.any(Function));

    handler?.({
      payload: {
        discovered: 1,
        ingested: 0,
        skipped: 0,
        failed: 0,
        enriched: 0,
        done: true,
      },
    });
    expect(seen).toHaveLength(1);
  });
});
