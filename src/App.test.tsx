import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ScanProgress } from "./ipc";

const events = vi.hoisted(() => ({ scan: null as null | ((p: ScanProgress) => void) }));

vi.mock("./ipc", () => ({
  HIGHLIGHT_START: "\u{e000}",
  HIGHLIGHT_END: "\u{e001}",
  listDetectedAgents: vi.fn(),
  listRepositories: vi.fn(),
  listFolders: vi.fn(),
  listSessionsPage: vi.fn(),
  listRepositorySessionsPage: vi.fn(),
  listFolderSessionsPage: vi.fn(),
  listSessionMessagesPage: vi.fn(),
  getSession: vi.fn(),
  getGitSnapshot: vi.fn(),
  sessionSecretCount: vi.fn(),
  getFilePatch: vi.fn(),
  searchPage: vi.fn(),
  rescan: vi.fn(),
  onScanProgress: vi.fn(async (handler: (p: ScanProgress) => void) => {
    events.scan = handler;
    return () => {};
  }),
  chooseAgentRootDirectory: vi.fn(),
  addAgentRoot: vi.fn(),
  removeAgentRoot: vi.fn(),
  createFolder: vi.fn(),
  renameFolder: vi.fn(),
  deleteFolder: vi.fn(),
  setSessionFolder: vi.fn(),
  exportSessionMarkdown: vi.fn(),
  saveSessionExport: vi.fn(),
  forgetSession: vi.fn(),
  forgetEverything: vi.fn(),
  getBackupSchedule: vi.fn(),
  setBackupSchedule: vi.fn(),
  backupNow: vi.fn(),
  getSetting: vi.fn(),
  getJsonSetting: vi.fn(),
  setSetting: vi.fn(),
}));

import App from "./App";
import * as ipc from "./ipc";
import type { DetectedAgent, MessageDto, SearchHit, SessionDetail, SessionSummary } from "./ipc";

const m = vi.mocked(ipc);

function session(id: string, title: string, agent = "claude-code"): SessionSummary {
  return {
    id,
    agent_id: agent,
    title,
    started_at: null,
    ended_at: null,
    message_count: 3,
    tool_call_count: 1,
    primary_model: "claude-sonnet-4-5",
    parse_status: "ok",
    total_input_tokens: null,
    total_output_tokens: null,
    total_cache_tokens: null,
    est_cost_usd: null,
  };
}

function message(seq: number, role: string, parts: MessageDto["parts"]): MessageDto {
  return { id: `m${seq}`, seq, role, event_kind: "message", is_sidechain: false, ts: null, model: null, parts };
}

function detail(summary: SessionSummary): SessionDetail {
  return {
    summary,
    parse_note: null,
    segments: [],
    messages: [
      message(1, "user", [{ ordinal: 0, kind: "text", text: "Make the retry backoff deterministic.", content_json: null, searchable: true }]),
      message(2, "assistant", [
        {
          ordinal: 0,
          kind: "tool_use",
          text: null,
          content_json: JSON.stringify({ type: "tool_use", name: "Bash", input: { command: "cargo test sync" } }),
          searchable: false,
        },
      ]),
    ],
    file_events: [
      {
        id: "fe1",
        path: "src/sync/worker.rs",
        change_kind: "edit",
        old_path: null,
        lines_added: 1,
        lines_removed: 1,
        source: "agent_patch",
        has_patch: true,
      },
    ],
    next_message_cursor: null,
  };
}

function hit(sessionId: string, title: string, snippet: string): SearchHit {
  return {
    session_id: sessionId,
    source_kind: "message_part",
    source_id: `p-${sessionId}`,
    field: "text",
    snippet,
    rank: -1,
    title,
    agent_id: "codex",
    started_at: null,
  };
}

const agents: DetectedAgent[] = [
  { id: "claude-code", display_name: "Claude Code", installed: true, version: null, session_count: 2, roots: ["~/.claude/projects"], custom_roots: [] },
];

const alpha = session("a", "Fix flaky retry backoff");
const beta = session("b", "Port dotfiles to nix", "codex");

beforeEach(() => {
  m.listDetectedAgents.mockResolvedValue(agents);
  m.listRepositories.mockResolvedValue([
    { id: "repo-lore", display_name: "lore", identity_confidence: "confirmed", primary_path: null, is_missing: false, session_count: 1, worktree_count: 1 },
  ]);
  m.listFolders.mockResolvedValue([{ id: "f1", name: "Release prep", session_count: 0, position: 0 }]);
  m.listSessionsPage.mockResolvedValue({ sessions: [alpha, beta], next_cursor: null });
  m.listRepositorySessionsPage.mockResolvedValue({ sessions: [alpha], next_cursor: null });
  m.listFolderSessionsPage.mockResolvedValue({ sessions: [], next_cursor: null });
  m.getSession.mockImplementation(async (id) => detail(id === "b" ? beta : alpha));
  m.getGitSnapshot.mockResolvedValue([]);
  m.sessionSecretCount.mockResolvedValue(0);
  m.searchPage.mockResolvedValue({ hits: [], next_cursor: null });
  m.rescan.mockResolvedValue({ discovered: 0, ingested: 0, skipped: 0, failed: 0, enriched: 0 });
  m.getFilePatch.mockResolvedValue("@@ -1,2 +1,2 @@\n-let jitter = random();\n+let jitter = self.rng.gen();\n keep();\n");
  m.setSessionFolder.mockResolvedValue(undefined);
  m.forgetSession.mockResolvedValue({ blobs_removed: 2, source_paths: [] });
  m.getBackupSchedule.mockResolvedValue({ interval: "off", keep: 7 });
  m.getJsonSetting.mockResolvedValue(null);
  m.setSetting.mockResolvedValue(undefined);
});

afterEach(() => {
  vi.clearAllMocks();
  events.scan = null;
});

async function renderLoaded() {
  render(<App />);
  await screen.findByRole("option", { name: /Fix flaky retry backoff/ });
}

function typeQuery(text: string) {
  fireEvent.change(screen.getByRole("textbox", { name: "Search sessions" }), { target: { value: text } });
}

describe("browsing", () => {
  it("lists the newest sessions and opens one into the reader", async () => {
    await renderLoaded();
    fireEvent.click(screen.getByRole("option", { name: /Port dotfiles to nix/ }));

    const reader = await screen.findByRole("article", { name: "Port dotfiles to nix" });
    expect(within(reader).getByText("Make the retry backoff deterministic.")).toBeTruthy();
    expect(within(reader).getByText("cargo test sync")).toBeTruthy();
    expect(screen.getByRole("option", { name: /Port dotfiles to nix/ }).getAttribute("aria-selected")).toBe("true");
  });

  it("browses one repository from the sidebar", async () => {
    await renderLoaded();
    const sidebar = screen.getByRole("navigation", { name: "Sidebar" });
    fireEvent.click(within(sidebar).getByRole("button", { name: /^lore/ }));

    await waitFor(() => expect(screen.queryByRole("option", { name: /Port dotfiles/ })).toBeNull());
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("lore");
    expect(screen.getByText("1 session")).toBeTruthy();
    expect(within(sidebar).getByRole("button", { name: /^lore/ }).getAttribute("aria-current")).toBe("true");
    expect(m.listRepositorySessionsPage).toHaveBeenCalledWith("repo-lore", 100, null);
  });

  it("appends an older page when asked", async () => {
    m.listSessionsPage
      .mockResolvedValueOnce({ sessions: [alpha], next_cursor: "c1" })
      .mockResolvedValueOnce({ sessions: [beta], next_cursor: null });
    await renderLoaded();
    fireEvent.click(screen.getByRole("button", { name: "Load older sessions" }));

    await screen.findByRole("option", { name: /Port dotfiles to nix/ });
    expect(screen.getAllByRole("option").map((o) => o.textContent?.slice(0, 12))).toEqual(["Fix flaky re", "Port dotfile"]);
    expect(m.listSessionsPage).toHaveBeenLastCalledWith(100, "c1");
  });

  it("paints only the latest session when responses arrive out of order", async () => {
    let releaseFirst!: (d: SessionDetail) => void;
    m.getSession.mockImplementationOnce(() => new Promise((resolve) => (releaseFirst = resolve)));
    await renderLoaded();
    fireEvent.click(screen.getByRole("option", { name: /Fix flaky retry backoff/ }));
    fireEvent.click(screen.getByRole("option", { name: /Port dotfiles to nix/ }));
    await screen.findByRole("article", { name: "Port dotfiles to nix" });

    await act(async () => releaseFirst(detail(alpha)));
    expect(screen.getByRole("article", { name: "Port dotfiles to nix" })).toBeTruthy();
    expect(screen.queryByRole("article", { name: "Fix flaky retry backoff" })).toBeNull();
  });
});

describe("search", () => {
  it("runs a filtered query and highlights the matched words", async () => {
    m.searchPage.mockResolvedValue({
      hits: [hit("b", "Port dotfiles to nix", "the \u{e000}retry\u{e001} loop")],
      next_cursor: null,
    });
    await renderLoaded();
    typeQuery("retry agent:codex");

    const result = await screen.findByRole("option", { name: /Port dotfiles to nix/ });
    expect(within(result).getByText("retry").tagName).toBe("MARK");
    expect(within(result).getByText("in message")).toBeTruthy();
    expect(m.searchPage).toHaveBeenLastCalledWith("retry agent:codex", 50, null, "relevance");
  });

  it("scopes a search to the chosen repository and re-sorts it", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    fireEvent.click(within(screen.getByRole("navigation", { name: "Sidebar" })).getByRole("button", { name: /^lore/ }));
    typeQuery("retry");
    await screen.findByText("Searching sessions in lore.", { exact: false });
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("retry repo:repo-lore", 50, null, "relevance"));

    await user.click(screen.getByRole("combobox", { name: "Sort results" }));
    await user.click(await screen.findByRole("option", { name: "Newest" }));
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("retry repo:repo-lore", 50, null, "newest"));
    expect(screen.getByRole("combobox", { name: "Sort results" }).textContent).toBe("Newest");
  });

  it("lists sessions for a query of filters alone", async () => {
    m.searchPage.mockResolvedValue({ hits: [hit("b", "Port dotfiles to nix", "Port dotfiles to nix")], next_cursor: null });
    await renderLoaded();
    typeQuery("agent:codex has:error");

    expect(await screen.findByRole("option", { name: /Port dotfiles to nix/ })).toBeTruthy();
    expect(m.searchPage).toHaveBeenLastCalledWith("agent:codex has:error", 50, null, "relevance");
  });

  it("opens the filter reference from ? and inserts a filter", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    const field = screen.getByRole("textbox", { name: "Search sessions" });
    await user.click(field);
    expect(screen.queryByRole("dialog", { name: "Search filters" })).toBeNull();

    await user.keyboard("?");
    const reference = await screen.findByRole("dialog", { name: "Search filters" });
    await user.click(within(reference).getByRole("button", { name: /agent:codex/ }));
    expect((field as HTMLInputElement).value).toBe("agent:");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Search filters" })).toBeNull());
    expect(document.activeElement).toBe(field);
  });

  it("says when nothing matches", async () => {
    await renderLoaded();
    typeQuery("zzzqx");
    expect(await screen.findByText("No sessions match “zzzqx”.")).toBeTruthy();
  });

  it("reports a failed search in place", async () => {
    m.searchPage.mockRejectedValue(new Error("fts5: syntax error"));
    await renderLoaded();
    typeQuery("retry");
    expect(await screen.findByText("The search did not run.")).toBeTruthy();
    expect(screen.getByText("Error: fts5: syntax error")).toBeTruthy();
  });

  it("highlights the search words inside the opened session", async () => {
    m.searchPage.mockResolvedValue({ hits: [hit("a", "Fix flaky retry backoff", "\u{e000}backoff\u{e001}")], next_cursor: null });
    await renderLoaded();
    typeQuery("backoff");
    fireEvent.click(await screen.findByRole("option", { name: /Fix flaky retry backoff/ }));

    const reader = await screen.findByRole("article", { name: "Fix flaky retry backoff" });
    const marks = within(reader).getAllByText("backoff");
    expect(marks.map((el) => el.tagName)).toEqual(["MARK"]);
  });
});

describe("keyboard", () => {
  it("moves from the query line into the list, opens with Enter, and backs out with Escape", async () => {
    await renderLoaded();
    fireEvent.keyDown(document.body, { key: "/" });
    const query = screen.getByRole("textbox", { name: "Search sessions" });
    expect(document.activeElement).toBe(query);

    fireEvent.keyDown(query, { key: "ArrowDown" });
    const list = screen.getByRole("listbox", { name: "Sessions in All Sessions" });
    expect(document.activeElement).toBe(list);

    fireEvent.keyDown(list, { key: "ArrowDown" });
    fireEvent.keyDown(list, { key: "Enter" });
    const reader = await screen.findByRole("article", { name: "Port dotfiles to nix" });
    await waitFor(() => expect(document.activeElement).toBe(reader));

    fireEvent.keyDown(reader, { key: "Escape" });
    expect(document.activeElement).toBe(list);
    fireEvent.keyDown(list, { key: "k" });
    fireEvent.keyDown(list, { key: "ArrowUp" });
    expect(document.activeElement).toBe(query);
  });

  it("opens the palette with ⌘K and jumps to a session", async () => {
    await renderLoaded();
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    const input = await screen.findByRole("combobox", { name: /Jump to a session/ });
    fireEvent.change(input, { target: { value: "dotfiles" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(await screen.findByRole("article", { name: "Port dotfiles to nix" })).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
  });
});

describe("scan", () => {
  it("shows progress on the Rescan control and records when the scan finished", async () => {
    await renderLoaded();
    expect(screen.getByText("2 sessions")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Rescan" }));
    const scanning = await screen.findByRole("button", { name: "Scanning" });
    expect(screen.getByText("Scanning…")).toBeTruthy();

    act(() => events.scan?.({ discovered: 10, ingested: 2, skipped: 1, failed: 0, enriched: 0, done: false }));
    expect(screen.getByRole("progressbar", { name: "Archive scan" }).getAttribute("aria-valuetext")).toBe("3 of 10 sessions");
    act(() => scanning.focus());
    expect((await screen.findByRole("tooltip")).textContent).toBe("Scanning 3 of 10");

    act(() => events.scan?.({ discovered: 10, ingested: 8, skipped: 1, failed: 1, enriched: 3, done: true }));
    expect(screen.getByRole("status").textContent).toBe("Scan finished: 8 new or changed, 1 unchanged, 1 unreadable");
    expect(screen.getByText("2 sessions")).toBeTruthy();
    expect((await screen.findByRole("tooltip")).textContent).toBe("Rescan (⌘R), last scanned just now. 8 new or changed, 1 unchanged, 1 unreadable");
    const [key, stored] = m.setSetting.mock.calls[0];
    expect(key).toBe("ui.lastScanAt");
    expect(Date.now() - JSON.parse(stored)).toBeLessThan(5_000);
  });

  it("names the last scan time from the stored setting", async () => {
    m.getJsonSetting.mockResolvedValue(Date.now() - 2 * 60_000);
    await renderLoaded();
    act(() => screen.getByRole("button", { name: "Rescan" }).focus());
    expect((await screen.findByRole("tooltip")).textContent).toBe("Rescan (⌘R), last scanned 2 minutes ago");
    expect(m.getJsonSetting).toHaveBeenCalledWith("ui.lastScanAt");
  });

  it("rescans with ⌘R", async () => {
    await renderLoaded();
    fireEvent.keyDown(window, { key: "r", metaKey: true });
    expect(await screen.findByRole("button", { name: "Scanning" })).toBeTruthy();
    expect(m.rescan).toHaveBeenCalledTimes(1);
  });

  it("refreshes the list as the worker ingests sessions", async () => {
    await renderLoaded();
    m.listSessionsPage.mockResolvedValue({ sessions: [session("c", "A brand new session"), alpha, beta], next_cursor: null });
    act(() => events.scan?.({ discovered: 3, ingested: 1, skipped: 0, failed: 0, enriched: 0, done: false }));
    expect(await screen.findByRole("option", { name: /A brand new session/ }, { timeout: 2000 })).toBeTruthy();
  });

  it("reports a rescan that could not start, with a way to try again", async () => {
    const user = userEvent.setup();
    m.rescan.mockRejectedValue("background ingestion worker unavailable");
    await renderLoaded();
    fireEvent.click(screen.getByRole("button", { name: "Rescan" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Scan failed: background ingestion worker unavailable");

    await user.click(screen.getByRole("button", { name: "Scan failed" }));
    const popover = screen.getByRole("dialog", { name: "Scan failed" });
    expect(within(popover).getByText("background ingestion worker unavailable")).toBeTruthy();
    await user.click(within(popover).getByRole("button", { name: "Try Again" }));
    expect(m.rescan).toHaveBeenCalledTimes(2);
  });
});

describe("states", () => {
  it("never shows a failed first load as an empty archive, and recovers on retry", async () => {
    m.listRepositories.mockRejectedValueOnce(new Error("database is locked"));
    render(<App />);
    expect(await screen.findByText("Lore could not open the archive.")).toBeTruthy();
    expect(screen.queryByRole("heading", { name: /searchable place/ })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("option", { name: /Fix flaky retry backoff/ })).toBeTruthy();
  });

  it("offers first-run setup for a truly empty archive", async () => {
    m.listDetectedAgents.mockResolvedValue([{ ...agents[0], session_count: 0 }]);
    m.listRepositories.mockResolvedValue([]);
    m.listSessionsPage.mockResolvedValue({ sessions: [], next_cursor: null });
    render(<App />);

    expect(await screen.findByRole("heading", { name: "Every agent session you have run, in one searchable place." })).toBeTruthy();
    expect(screen.getByText("~/.claude/projects")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Scan agent history" }));
    expect(await screen.findByRole("button", { name: "Scanning agent history…" })).toBeTruthy();
    expect(m.rescan).toHaveBeenCalledTimes(1);
  });

  it("shows a session that failed to open, with a retry", async () => {
    m.getSession.mockRejectedValueOnce(new Error("database is locked"));
    await renderLoaded();
    fireEvent.click(screen.getByRole("option", { name: /Fix flaky retry backoff/ }));
    expect(await screen.findByText("Lore could not open this session.")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("article", { name: "Fix flaky retry backoff" })).toBeTruthy();
  });
});

describe("session actions", () => {
  it("loads and renders a recorded patch with line numbers", async () => {
    await renderLoaded();
    fireEvent.click(screen.getByRole("option", { name: /Fix flaky retry backoff/ }));
    await screen.findByRole("article", { name: "Fix flaky retry backoff" });
    fireEvent.click(screen.getByRole("tab", { name: /Files/ }));
    fireEvent.click(screen.getByRole("button", { name: "Show patch" }));

    const patch = await screen.findByRole("figure", { name: "Patch for src/sync/worker.rs" });
    const rows = within(patch).getAllByRole("row").map((row) => row.textContent);
    expect(rows).toEqual(["@@ -1,2 +1,2 @@", "1-let jitter = random();", "1+let jitter = self.rng.gen();", "22 keep();"]);
    expect(m.getFilePatch).toHaveBeenCalledWith("fe1");
  });

  it("files the open session into a folder", async () => {
    await renderLoaded();
    fireEvent.click(screen.getByRole("option", { name: /Fix flaky retry backoff/ }));
    await screen.findByRole("article", { name: "Fix flaky retry backoff" });
    fireEvent.change(screen.getByRole("combobox", { name: "File in folder" }), { target: { value: "f1" } });

    expect(await screen.findByText("Filed in “Release prep”.")).toBeTruthy();
    expect(m.setSessionFolder).toHaveBeenCalledWith("a", "f1");
  });

  it("forgets a session only after confirming", async () => {
    await renderLoaded();
    fireEvent.click(screen.getByRole("option", { name: /Fix flaky retry backoff/ }));
    await screen.findByRole("article", { name: "Fix flaky retry backoff" });
    fireEvent.click(screen.getByRole("button", { name: "Forget…" }));

    const dialog = screen.getByRole("alertdialog", { name: "Forget “Fix flaky retry backoff”?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(m.forgetSession).not.toHaveBeenCalled();
    expect(screen.getByRole("article", { name: "Fix flaky retry backoff" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Forget…" }));
    fireEvent.click(screen.getByRole("button", { name: "Forget session" }));
    expect(await screen.findByText("Forgot “Fix flaky retry backoff”.")).toBeTruthy();
    expect(m.forgetSession).toHaveBeenCalledWith("a");
    expect(screen.getByText("Pick a session to read it.")).toBeTruthy();
  });
});

describe("window shell", () => {
  function sidebar() {
    return screen.getByRole("navigation", { name: "Sidebar" });
  }

  it("has no status bar; the sidebar carries the counts", async () => {
    await renderLoaded();
    expect(document.querySelector("footer")).toBeNull();
    expect(within(sidebar()).getByRole("button", { name: /All Sessions/ }).textContent).toBe("All Sessions2");
    expect(within(sidebar()).getByRole("button", { name: /Claude Code/ }).textContent).toBe("Claude Code2");
    expect(within(sidebar()).getByRole("button", { name: /Release prep/ }).textContent).toBe("Release prep0");
  });

  it("filters to one agent from its sidebar row and back", async () => {
    m.searchPage.mockResolvedValue({ hits: [hit("a", "Fix flaky retry backoff", "Fix flaky retry backoff")], next_cursor: null });
    await renderLoaded();
    fireEvent.click(within(sidebar()).getByRole("button", { name: /Claude Code/ }));

    expect((screen.getByRole("textbox", { name: "Search sessions" }) as HTMLInputElement).value).toBe("agent:claude-code");
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("agent:claude-code", 50, null, "relevance"));
    expect(within(sidebar()).getByRole("button", { name: /Claude Code/ }).getAttribute("aria-current")).toBe("true");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Claude Code");

    fireEvent.click(within(sidebar()).getByRole("button", { name: /All Sessions/ }));
    expect((screen.getByRole("textbox", { name: "Search sessions" }) as HTMLInputElement).value).toBe("");
    expect(await screen.findByRole("option", { name: /Port dotfiles to nix/ })).toBeTruthy();
  });

  it("moves the sidebar selection with the arrow keys", async () => {
    await renderLoaded();
    const all = within(sidebar()).getByRole("button", { name: /All Sessions/ });
    act(() => all.focus());
    fireEvent.keyDown(all, { key: "ArrowDown" });

    const repo = within(sidebar()).getByRole("button", { name: /^lore/ });
    expect(document.activeElement).toBe(repo);
    expect(repo.getAttribute("aria-current")).toBe("true");
    await waitFor(() => expect(m.listRepositorySessionsPage).toHaveBeenCalledWith("repo-lore", 100, null));
  });

  it("hides the sidebar with ⌃⌘S and keeps every scope in the title menu", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    fireEvent.keyDown(window, { key: "s", metaKey: true, ctrlKey: true });
    expect(screen.queryByRole("navigation", { name: "Sidebar" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "All Sessions, choose what to browse" }));
    expect(screen.getAllByRole("menuitemradio").map((item) => item.textContent)).toEqual([
      "All Sessions2",
      "lore1",
      "Release prep0",
      "Claude Code2",
    ]);
    await user.click(screen.getByRole("menuitemradio", { name: /^lore/ }));
    await waitFor(() => expect(m.listRepositorySessionsPage).toHaveBeenCalledWith("repo-lore", 100, null));
    expect(screen.getByRole("button", { name: "lore, choose what to browse" })).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Show Sidebar" }));
    expect(sidebar()).toBeTruthy();
  });

  it("starts with the sidebar hidden in a narrow window", async () => {
    const matchMedia = vi.fn((query: string) => ({
      matches: query === "(max-width: 999px)",
      addEventListener: () => {},
      removeEventListener: () => {},
    }));
    vi.stubGlobal("matchMedia", matchMedia);
    try {
      await renderLoaded();
      expect(screen.queryByRole("navigation", { name: "Sidebar" })).toBeNull();
      expect(screen.getByRole("button", { name: "All Sessions, choose what to browse" })).toBeTruthy();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("renames a folder from its context menu and deletes it from the palette", async () => {
    const user = userEvent.setup();
    m.renameFolder.mockResolvedValue(undefined);
    m.deleteFolder.mockResolvedValue(undefined);
    await renderLoaded();
    const folder = within(sidebar()).getByRole("button", { name: /Release prep/ });
    await user.pointer({ keys: "[MouseRight]", target: folder });
    await user.click(screen.getByRole("menuitem", { name: "Rename Folder…" }));
    const rename = screen.getByRole("dialog", { name: "Rename Folder" });
    const name = within(rename).getByRole("textbox");
    expect((name as HTMLInputElement).value).toBe("Release prep");
    await user.clear(name);
    await user.type(name, "Shipped{Enter}");
    await waitFor(() => expect(m.renameFolder).toHaveBeenCalledWith("f1", "Shipped"));

    fireEvent.click(within(sidebar()).getByRole("button", { name: /Release prep/ }));
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    const input = await screen.findByRole("combobox", { name: /Jump to a session/ });
    fireEvent.change(input, { target: { value: "Delete “Release prep”" } });
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.click(within(screen.getByRole("alertdialog", { name: "Delete “Release prep”?" })).getByRole("button", { name: "Delete Folder" }));
    await waitFor(() => expect(m.deleteFolder).toHaveBeenCalledWith("f1"));
    expect(await screen.findByText("Deleted folder “Release prep”.")).toBeTruthy();
  });

  it("creates a folder from the sidebar", async () => {
    m.createFolder.mockResolvedValue({ id: "f2", name: "Ideas", session_count: 0, position: 1 });
    await renderLoaded();
    fireEvent.click(within(sidebar()).getByRole("button", { name: "New Folder" }));
    const dialog = screen.getByRole("dialog", { name: "New Folder" });
    fireEvent.change(within(dialog).getByRole("textbox"), { target: { value: "Ideas" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create folder" }));
    await waitFor(() => expect(m.createFolder).toHaveBeenCalledWith("Ideas"));
  });

  it("reaches Settings by mouse through the palette button", async () => {
    await renderLoaded();
    fireEvent.click(screen.getByRole("button", { name: "Command Palette" }));
    const input = await screen.findByRole("combobox", { name: /Jump to a session/ });
    fireEvent.change(input, { target: { value: "Settings" } });
    fireEvent.click(screen.getByRole("option", { name: /^Settings/ }));
    expect(await screen.findByRole("dialog", { name: /Settings/ })).toBeTruthy();
  });

  it("shows a notice under the toolbar until it is dismissed", async () => {
    await renderLoaded();
    fireEvent.click(screen.getByRole("option", { name: /Fix flaky retry backoff/ }));
    await screen.findByRole("article", { name: "Fix flaky retry backoff" });
    fireEvent.change(screen.getByRole("combobox", { name: "File in folder" }), { target: { value: "f1" } });
    expect((await screen.findByText("Filed in “Release prep”.")).getAttribute("role")).toBe("status");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText("Filed in “Release prep”.")).toBeNull();
  });
});
