import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const listen = vi.fn();
const open = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: (...a: unknown[]) => listen(...a) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: (...a: unknown[]) => open(...a) }));

import {
  addAgentRoot,
  chooseAgentRootDirectory,
  HIGHLIGHT_END,
  HIGHLIGHT_START,
  listDetectedAgents,
  listFolderSessionsPage,
  listRepositorySessionsPage,
  listSessions,
  listSessionsPage,
  onScanProgress,
  removeAgentRoot,
  rescan,
} from "./ipc";
import source from "./ipc?raw";
import { createMockHandlers } from "./mock/ipc-mock";

beforeEach(() => {
  invoke.mockReset();
  listen.mockReset();
  open.mockReset();
});

describe("ipc contract", () => {
  it("list_detected_agents invokes its command", async () => {
    invoke.mockResolvedValue([]);
    await listDetectedAgents();
    expect(invoke).toHaveBeenCalledWith("list_detected_agents");
  });

  it("agent-root commands preserve the selected local path", async () => {
    invoke.mockResolvedValue(undefined);
    await addAgentRoot("codex", "/Volumes/archive/codex");
    await removeAgentRoot("codex", "/Volumes/archive/codex");

    expect(invoke).toHaveBeenNthCalledWith(1, "add_agent_root", {
      agentId: "codex",
      path: "/Volumes/archive/codex",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "remove_agent_root", {
      agentId: "codex",
      path: "/Volumes/archive/codex",
    });
  });

  it("folder selection uses a single-directory native dialog", async () => {
    open.mockResolvedValue("/Volumes/archive/codex");
    await expect(chooseAgentRootDirectory("Codex")).resolves.toBe(
      "/Volumes/archive/codex",
    );
    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "Choose Codex session folder",
    });
  });

  it("list_sessions defaults limit to 50", async () => {
    invoke.mockResolvedValue([]);
    await listSessions();
    expect(invoke).toHaveBeenCalledWith("list_sessions", { limit: 50 });

    await listSessions(10);
    expect(invoke).toHaveBeenCalledWith("list_sessions", { limit: 10 });
  });

  it("session page commands pass opaque cursors unchanged", async () => {
    invoke.mockResolvedValue({ sessions: [], next_cursor: null });
    await listSessionsPage(100, "all-cursor");
    await listRepositorySessionsPage("repo-a", 100, "repo-cursor");
    await listFolderSessionsPage("folder-1", 100, "folder-cursor");
    await listFolderSessionsPage("folder-1", 50);

    expect(invoke).toHaveBeenNthCalledWith(1, "list_sessions_page", {
      limit: 100,
      cursor: "all-cursor",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "list_repository_sessions_page", {
      id: "repo-a",
      limit: 100,
      cursor: "repo-cursor",
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "list_folder_sessions_page", {
      id: "folder-1",
      limit: 100,
      cursor: "folder-cursor",
    });
    expect(invoke).toHaveBeenNthCalledWith(4, "list_folder_sessions_page", {
      id: "folder-1",
      limit: 50,
      cursor: null,
    });
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

  it("exportSessionMarkdown defaults includeSecrets to false", async () => {
    invoke.mockResolvedValue("# Session");
    await (await import("./ipc")).exportSessionMarkdown("s1");
    expect(invoke).toHaveBeenCalledWith("export_session_markdown", {
      id: "s1",
      includeSecrets: false,
    });
  });

  it("setBackupSchedule passes interval and keep count, defaulting keep to 7", async () => {
    invoke.mockResolvedValue(undefined);
    const interval: import("./ipc").BackupInterval = "weekly";
    await (await import("./ipc")).setBackupSchedule(interval, 14);
    expect(invoke).toHaveBeenNthCalledWith(1, "set_backup_schedule", {
      interval: "weekly",
      keep: 14,
    });

    await (await import("./ipc")).setBackupSchedule("daily");
    expect(invoke).toHaveBeenNthCalledWith(2, "set_backup_schedule", {
      interval: "daily",
      keep: 7,
    });
  });

  it("searchPage invokes with defaults and custom sort order", async () => {
    invoke.mockResolvedValue({ hits: [], next_cursor: null });
    const { searchPage } = await import("./ipc");
    const sort: import("./ipc").SearchSort = "newest";
    await searchPage("test query", 25, "c1", sort);
    expect(invoke).toHaveBeenCalledWith("search_page", {
      query: "test query",
      limit: 25,
      cursor: "c1",
      sort: "newest",
    });
  });

  it("getJsonSetting and setJsonSetting serialize and parse typed payloads", async () => {
    const { getJsonSetting, setJsonSetting } = await import("./ipc");
    invoke.mockResolvedValueOnce(JSON.stringify({ enabled: true, count: 42 }));
    const parsed = await getJsonSetting<{ enabled: boolean; count: number }>("test.key");
    expect(parsed).toEqual({ enabled: true, count: 42 });
    expect(invoke).toHaveBeenCalledWith("get_setting", { key: "test.key" });

    invoke.mockResolvedValueOnce(null);
    expect(await getJsonSetting("missing.key")).toBeNull();

    invoke.mockResolvedValueOnce("invalid json {{{");
    expect(await getJsonSetting("malformed.key")).toBeNull();

    invoke.mockResolvedValueOnce(undefined);
    await setJsonSetting("test.key", { enabled: false });
    expect(invoke).toHaveBeenCalledWith("set_setting", {
      key: "test.key",
      valueJson: JSON.stringify({ enabled: false }),
    });

    invoke.mockResolvedValueOnce(undefined);
    await setJsonSetting("undefined.key", undefined);
    expect(invoke).toHaveBeenCalledWith("set_setting", {
      key: "undefined.key",
      valueJson: "null",
    });
  });

  it("exports strongly-typed domain union types", async () => {
    const { HIGHLIGHT_START, HIGHLIGHT_END } = await import("./ipc");
    expect(HIGHLIGHT_START).toBe("\u{e000}");
    expect(HIGHLIGHT_END).toBe("\u{e001}");

    const scanState: import("./ipc").SecretScanState = "failed_quarantined";
    expect(scanState).toBe("failed_quarantined");

    const severity: import("./ipc").SecretSeverity = "critical";
    expect(severity).toBe("critical");

    const sourceKind: import("./ipc").SearchSourceKind = "message_part";
    expect(sourceKind).toBe("message_part");

    const sortOrder: import("./ipc").SearchSortOrder = "relevance";
    expect(sortOrder).toBe("relevance");

    const hitField: import("./ipc").SearchHitField = "title";
    expect(hitField).toBe("title");
  });

  it("getTokenTotals, relinkSegmentRepository, and checkForUpdates invoke expected commands", async () => {
    const { checkForUpdates, getTokenTotals, relinkSegmentRepository } = await import("./ipc");

    invoke.mockResolvedValueOnce({
      total_input_tokens: 100,
      total_output_tokens: 20,
      total_cache_tokens: 10,
      est_cost_usd: 0.05,
    });
    const totals = await getTokenTotals();
    expect(invoke).toHaveBeenCalledWith("get_token_totals");
    expect(totals.total_input_tokens).toBe(100);

    invoke.mockResolvedValueOnce(undefined);
    await relinkSegmentRepository("seg-1", "repo-2");
    expect(invoke).toHaveBeenCalledWith("relink_segment_repository", {
      segmentId: "seg-1",
      repositoryId: "repo-2",
    });

    invoke.mockResolvedValueOnce({
      update_available: false,
      current_version: "0.1.0",
      latest_version: null,
      release_notes: null,
      published_at: null,
    });
    const updateResult = await checkForUpdates();
    expect(invoke).toHaveBeenCalledWith("check_for_updates");
    expect(updateResult.current_version).toBe("0.1.0");
  });
});

// The browser-only sample-data mock must answer every command `ipc.ts` sends.
// The commands are read from the source, so a new `invoke("...")` without a
// handler in `src/mock/ipc-mock.ts` fails here.
describe("browser sample-data mock", () => {
  const commands = [
    ...new Set([...source.matchAll(/invoke(?:<[^(]*>)?\(\s*"([a-z_]+)"/g)].map((m) => m[1])),
  ];
  const events = [
    ...new Set([...source.matchAll(/listen(?:<[^(]*>)?\(\s*"([a-z_]+)"/g)].map((m) => m[1])),
  ];

  function makeMock() {
    const emitted: Array<[string, unknown]> = [];
    const handlers = createMockHandlers((event, payload) => emitted.push([event, payload]), {
      now: Date.UTC(2026, 0, 1),
      scanStepMs: 0,
    });
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const call = async (cmd: string, args: Record<string, unknown> = {}): Promise<any> =>
      handlers[cmd](args);
    return { handlers, emitted, call };
  }

  it("finds the commands and events ipc.ts uses", () => {
    expect(commands).toContain("list_sessions_page");
    expect(commands).toContain("rescan");
    expect(events).toContain("scan_progress");
  });

  it("has a handler for every command ipc.ts invokes", () => {
    const { handlers } = makeMock();
    expect(commands.filter((cmd) => typeof handlers[cmd] !== "function")).toEqual([]);
  });

  it("wraps search matches in the backend's highlight markers", async () => {
    const { call } = makeMock();
    const page = await call("search_page", { query: "rounding", limit: 50, cursor: null, sort: "relevance" });
    expect(page.hits.length).toBeGreaterThan(0);
    for (const hit of page.hits) {
      expect(hit.snippet).toContain(HIGHLIGHT_START);
      expect(hit.snippet).toContain(HIGHLIGHT_END);
    }
  });

  it("returns nothing for a word that is not in the sample data", async () => {
    const { call } = makeMock();
    const page = await call("search_page", { query: "zzyzxnotaword", limit: 50, cursor: null, sort: "relevance" });
    expect(page).toEqual({ hits: [], next_cursor: null });
  });

  it("pages search results without repeats and rejects a cursor from another query", async () => {
    const { call } = makeMock();
    const args = { query: "tests", limit: 50, sort: "relevance" };
    const first = await call("search_page", { ...args, cursor: null });
    expect(first.next_cursor).not.toBeNull();
    const second = await call("search_page", { ...args, cursor: first.next_cursor });
    expect(second.hits.length).toBeGreaterThan(0);
    const ids = [...first.hits, ...second.hits].map((h: { source_id: string }) => h.source_id);
    expect(new Set(ids).size).toBe(ids.length);
    await expect(
      call("search_page", { ...args, query: "other", cursor: first.next_cursor }),
    ).rejects.toThrow(/cursor/);
  });

  it("pages sessions newest first by cursor", async () => {
    const { call } = makeMock();
    const first = await call("list_sessions_page", { limit: 100, cursor: null });
    const second = await call("list_sessions_page", { limit: 100, cursor: first.next_cursor });
    expect(first.sessions).toHaveLength(100);
    expect(second.sessions.length).toBeGreaterThan(0);
    const ids = [...first.sessions, ...second.sessions].map((s: { id: string }) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("keeps settings and the backup schedule in memory", async () => {
    const { call } = makeMock();
    await call("set_setting", { key: "theme", valueJson: '"dark"' });
    expect(await call("get_setting", { key: "theme" })).toBe('"dark"');
    await call("set_backup_schedule", { interval: "daily", keep: 3 });
    expect(await call("get_backup_schedule")).toEqual({ interval: "daily", keep: 3 });
  });

  it("emits scan_progress events during a rescan and ends with done", async () => {
    const { call, emitted } = makeMock();
    const result = await call("rescan");
    const progress = emitted
      .filter(([event]) => event === "scan_progress")
      .map(([, payload]) => payload as { done: boolean });
    expect(progress.length).toBeGreaterThan(2);
    expect(progress[progress.length - 1].done).toBe(true);
    expect(result.discovered).toBeGreaterThan(0);
  });

  it("restores the sample sessions on rescan after forget everything", async () => {
    const { call } = makeMock();
    await call("forget_everything");
    expect((await call("list_sessions_page", { limit: 10, cursor: null })).sessions).toEqual([]);
    const result = await call("rescan");
    expect(result.ingested).toBe(result.discovered);
    expect((await call("list_sessions_page", { limit: 10, cursor: null })).sessions).toHaveLength(10);
  });
});
