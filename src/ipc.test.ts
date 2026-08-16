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
  listDetectedAgents,
  listFolderSessionsPage,
  listRepositorySessionsPage,
  listSessions,
  listSessionsPage,
  onScanProgress,
  removeAgentRoot,
  rescan,
} from "./ipc";

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
  });
});
