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
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.density;
});

async function renderLoaded() {
  render(<App />);
  await screen.findByRole("option", { name: /Fix flaky retry backoff/ });
}

function typeQuery(text: string) {
  fireEvent.change(screen.getByRole("combobox", { name: "Search sessions" }), { target: { value: text } });
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

    const list = screen.getByRole("listbox", { name: "Sessions in All Sessions" });
    const older = await within(list).findByRole("option", { name: /Port dotfiles to nix/ });
    expect(within(list).getAllByRole("option").map((o) => o.textContent?.slice(0, 12))).toEqual(["Fix flaky re", "Port dotfile"]);
    expect(m.listSessionsPage).toHaveBeenLastCalledWith(100, "c1");
    // The first newly loaded row takes the selection and the list keeps focus.
    await waitFor(() => expect(older.getAttribute("aria-selected")).toBe("true"));
    expect(document.activeElement).toBe(list);
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
    expect(result.textContent).toBe("Port dotfiles to nixthe retry loop");
    expect(m.searchPage).toHaveBeenLastCalledWith("retry agent:codex", 50, null, "relevance");
  });

  it("scopes a search to the chosen repository and re-sorts it", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    fireEvent.click(within(screen.getByRole("navigation", { name: "Sidebar" })).getByRole("button", { name: /^lore/ }));
    typeQuery("retry");
    const scopeBar = screen.getByRole("radiogroup", { name: "Search:" });
    expect(within(scopeBar).getByRole("radio", { name: "lore" }).getAttribute("aria-checked")).toBe("true");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Searching “lore”");
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("retry repo:repo-lore", 50, null, "relevance"));

    await user.click(screen.getByRole("button", { name: "Sort results, Best Match" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Newest" }));
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("retry repo:repo-lore", 50, null, "newest"));
    expect(screen.getByRole("button", { name: "Sort results, Newest" })).toBeTruthy();
  });

  it("lists sessions for a query of filters alone", async () => {
    m.searchPage.mockResolvedValue({ hits: [hit("b", "Port dotfiles to nix", "Port dotfiles to nix")], next_cursor: null });
    await renderLoaded();
    typeQuery("agent:codex has:error");

    expect(await screen.findByRole("option", { name: /Port dotfiles to nix/ })).toBeTruthy();
    expect(m.searchPage).toHaveBeenLastCalledWith("agent:codex has:error", 50, null, "relevance");
  });

  it("lists the matching sessions for agent:codex alone, newest first as the backend orders them", async () => {
    const newer = { ...hit("b", "Port dotfiles to nix", "Port dotfiles to nix"), rank: 0 };
    const older = { ...hit("c", "Bump the codex sandbox", "Bump the codex sandbox"), rank: 0 };
    m.searchPage.mockImplementation(async (query) =>
      query === "agent:codex" ? { hits: [newer, older], next_cursor: null } : { hits: [], next_cursor: null },
    );
    await renderLoaded();
    typeQuery("agent:codex");

    const results = await screen.findByRole("listbox", { name: "Search results" });
    expect(within(results).getAllByRole("option").map((o) => o.textContent)).toEqual(["Port dotfiles to nixCodex", "Bump the codex sandboxCodex"]);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Searching “All Sessions”");
    expect(screen.getByText("2 sessions")).toBeTruthy();
    expect(m.searchPage).toHaveBeenLastCalledWith("agent:codex", 50, null, "relevance");
  });

  it("shows one row per session with how many more matches it has", async () => {
    const hits = [
      hit("a", "Fix flaky retry backoff", "the \u{e000}retry\u{e001} budget"),
      { ...hit("a", "Fix flaky retry backoff", "\u{e000}retry\u{e001} once more"), source_id: "p-a2" },
      hit("b", "Port dotfiles to nix", "a \u{e000}retry\u{e001} wrapper"),
      // Raw tool-call JSON never becomes a row of its own.
      { ...hit("c", "Bump the codex sandbox", '{"name":"Bash","input":{"command":"\u{e000}retry\u{e001}"}}'), field: "content_json" },
      { ...hit("a", "Fix flaky retry backoff", "last \u{e000}retry\u{e001}"), source_id: "p-a3" },
    ];
    m.searchPage.mockImplementation(async (query) => ({ hits, next_cursor: query === "retry" ? null : "c1" }));
    await renderLoaded();
    typeQuery("retry");

    const results = await screen.findByRole("listbox", { name: "Search results" });
    expect(within(results).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "Fix flaky retry backoffthe retry budget2 more matches",
      "Port dotfiles to nixa retry wrapper",
    ]);
    expect(screen.getByText("2 sessions")).toBeTruthy();

    // With pages still to load, neither count is exact.
    typeQuery("backoff");
    expect(await screen.findByText("More matches in this session")).toBeTruthy();
    expect(screen.getByText("More than 2 sessions")).toBeTruthy();
  });

  it("opens the filter reference from ? and inserts a filter", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    const field = screen.getByRole("combobox", { name: "Search sessions" });
    await user.click(field);
    expect(screen.queryByRole("dialog", { name: "Search filters" })).toBeNull();

    await user.keyboard("?");
    const reference = await screen.findByRole("dialog", { name: "Search filters" });
    expect(within(reference).getByRole("button", { name: /repo:lore/ })).toBeTruthy();
    expect(within(reference).getByRole("button", { name: /worktree:feat/ })).toBeTruthy();
    expect(within(reference).getByRole("radio", { name: "Best Match" }).getAttribute("aria-checked")).toBe("true");
    await user.click(within(reference).getByRole("button", { name: /agent:codex/ }));
    expect((field as HTMLInputElement).value).toBe("agent:");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Search filters" })).toBeNull());
    expect(document.activeElement).toBe(field);
  });

  it("turns a finished filter into a token and deletes it with two Backspaces", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    const field = screen.getByRole("combobox", { name: "Search sessions" }) as HTMLInputElement;
    await user.click(field);
    await user.keyboard("has:patch retry");

    const search = screen.getByRole("search");
    expect(search.textContent).toBe("has:patch");
    expect(within(search).getByText("has:").className).toContain("font-mono");
    expect(field.value).toBe("retry");
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("has:patch retry", 50, null, "relevance"));

    field.setSelectionRange(0, 0);
    fireEvent.keyDown(field, { key: "Backspace" });
    expect(within(search).getByText("has:").parentElement?.hasAttribute("data-selected")).toBe(true);
    expect(field.value).toBe("retry");
    fireEvent.keyDown(field, { key: "Backspace" });
    expect(search.textContent).toBe("");
    expect(field.value).toBe("retry");
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("retry", 50, null, "relevance"));

    await user.click(within(search).getByRole("button", { name: "Clear Search" }));
    expect(field.value).toBe("");
    expect(document.activeElement).toBe(field);
  });

  it("offers values after a bare key and picks one with the keyboard", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    const field = screen.getByRole("combobox", { name: "Search sessions" }) as HTMLInputElement;
    await user.click(field);
    expect(screen.queryByRole("dialog")).toBeNull();

    await user.keyboard("agent:");
    const values = await screen.findByRole("listbox", { name: "Values for agent:" });
    const options = within(values).getAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual(["agent:claude-code2", "Filter Reference⌘/"]);
    expect(field.getAttribute("aria-activedescendant")).toBe(options[0].id);
    await user.keyboard("{ArrowDown}");
    expect(options[1].getAttribute("aria-selected")).toBe("true");
    await user.keyboard("{ArrowUp}{Enter}");

    expect(field.value).toBe("");
    expect(screen.getByRole("search").textContent).toBe("agent:claude-code");
    expect(screen.queryByRole("listbox", { name: "Values for agent:" })).toBeNull();
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("agent:claude-code", 50, null, "relevance"));
  });

  it("leaves focus on the list when a row is clicked while the filter reference is open", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    await user.click(screen.getByRole("button", { name: "Search Filters" }));
    expect(await screen.findByRole("dialog", { name: "Search filters" })).toBeTruthy();

    await user.click(screen.getByRole("option", { name: /Port dotfiles to nix/ }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Search filters" })).toBeNull());
    expect(document.activeElement).toBe(screen.getByRole("listbox", { name: "Sessions in All Sessions" }));
  });

  it("closes the suggestions with Escape or a click elsewhere, keeping the text", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    const field = screen.getByRole("combobox", { name: "Search sessions" }) as HTMLInputElement;
    await user.click(field);
    await user.keyboard("repo:");
    const values = await screen.findByRole("listbox", { name: "Values for repo:" });
    expect(within(values).getAllByRole("option")[0].textContent).toBe("repo:lore1");

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("listbox", { name: "Values for repo:" })).toBeNull());
    expect(field.value).toBe("repo:");

    await user.keyboard("l");
    expect(await screen.findByRole("listbox", { name: "Values for repo:" })).toBeTruthy();
    await user.click(await screen.findByText("No Results"));
    await waitFor(() => expect(screen.queryByRole("listbox", { name: "Values for repo:" })).toBeNull());
    expect(field.value).toBe("repo:l");
  });

  it("says when nothing matches", async () => {
    await renderLoaded();
    typeQuery("zzzqx");
    expect(await screen.findByText("No Results")).toBeTruthy();
    expect(screen.getByText("“zzzqx”")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Clear Filters" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Search All Sessions" })).toBeNull();
  });

  it("offers to search everything and to drop the filters when a scoped search finds nothing", async () => {
    await renderLoaded();
    fireEvent.click(within(screen.getByRole("navigation", { name: "Sidebar" })).getByRole("button", { name: /^lore/ }));
    typeQuery("zzzqx agent:codex");
    expect(await screen.findByText("No Results")).toBeTruthy();
    expect(screen.getByText("No Results").parentElement?.textContent).toContain("“zzzqx” with agent:codex");

    fireEvent.click(screen.getByRole("button", { name: "Clear Filters" }));
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("zzzqx repo:repo-lore", 50, null, "relevance"));
    fireEvent.click(await screen.findByRole("button", { name: "Search All Sessions" }));
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("zzzqx", 50, null, "relevance"));
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Searching “All Sessions”");
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

describe("selection", () => {
  it("fills the selected row blue only while the list has focus in the key window", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    const list = screen.getByRole("listbox", { name: "Sessions in All Sessions" });
    const row = screen.getByRole("option", { name: /Fix flaky retry backoff/ });
    expect(row.textContent).toBe("Fix flaky retry backoffClaude Code, 3 messages");

    await user.click(row);
    expect(row.getAttribute("aria-selected")).toBe("true");
    expect(list.hasAttribute("data-focused")).toBe(true);

    fireEvent.blur(window);
    expect(list.hasAttribute("data-focused")).toBe(false);
    fireEvent.focus(window);
    expect(list.hasAttribute("data-focused")).toBe(true);

    await user.click(screen.getByRole("combobox", { name: "Search sessions" }));
    expect(list.hasAttribute("data-focused")).toBe(false);
    expect(row.getAttribute("aria-selected")).toBe("true");
  });
});

describe("keyboard", () => {
  it("moves from the query line into the list, opens with Enter, and backs out with Escape", async () => {
    await renderLoaded();
    fireEvent.keyDown(document.body, { key: "/" });
    const query = screen.getByRole("combobox", { name: "Search sessions" });
    expect(document.activeElement).toBe(query);

    fireEvent.keyDown(query, { key: "ArrowDown" });
    const list = screen.getByRole("listbox", { name: "Sessions in All Sessions" });
    expect(document.activeElement).toBe(list);

    // Launch opened the newest session; arrows open each session in place, Enter moves into the reader.
    expect(await screen.findByRole("article", { name: "Fix flaky retry backoff" })).toBeTruthy();
    fireEvent.keyDown(list, { key: "ArrowDown" });
    expect(screen.getByRole("option", { name: /Port dotfiles to nix/ }).getAttribute("aria-selected")).toBe("true");
    const reader = await screen.findByRole("article", { name: "Port dotfiles to nix" });
    expect(document.activeElement).toBe(list);
    // The session is already open, so Enter only moves focus.
    fireEvent.keyDown(list, { key: "Enter" });
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
    const input = await screen.findByRole("combobox", { name: "Search sessions and actions" });
    fireEvent.change(input, { target: { value: "dotfiles" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(await screen.findByRole("article", { name: "Port dotfiles to nix" })).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: "Command Palette" })).toBeNull();
  });
});

describe("scan", () => {
  it("shows progress on the Rescan control and records when the scan finished", async () => {
    await renderLoaded();
    expect(screen.getByText("2 sessions")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Rescan" }));
    const scanning = await screen.findByRole("button", { name: "Scanning" });
    expect(screen.getByText("Scanning…")).toBeTruthy();
    expect(scanning.getAttribute("aria-disabled")).toBe("true");
    // aria-disabled grays the button; the busy spinner must not inherit that disabled color.
    expect(scanning.querySelector("svg")?.getAttribute("class")).toContain("text-label-2");

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
    const toolbar = () =>
      within(screen.getByRole("banner"))
        .getAllByRole("button")
        .map((b) => b.getAttribute("aria-label"));
    const reader = ["Copy as Markdown", "File in Folder", "Save as File…", "More"];
    expect(toolbar()).toEqual([...reader, "Rescan", "Command Palette", "Search Filters"]);
    fireEvent.click(screen.getByRole("button", { name: "Rescan" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Scan failed: background ingestion worker unavailable");
    expect(toolbar()).toEqual([...reader, "Scan failed", "Rescan", "Command Palette", "Search Filters"]);

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

  it("offers first-run setup for a truly empty archive, with search and scopes hidden", async () => {
    const user = userEvent.setup();
    m.listDetectedAgents.mockResolvedValue([{ ...agents[0], session_count: 0 }]);
    m.listRepositories.mockResolvedValue([]);
    m.listSessionsPage.mockResolvedValue({ sessions: [], next_cursor: null });
    m.chooseAgentRootDirectory.mockResolvedValue("/Volumes/old/claude");
    m.addAgentRoot.mockResolvedValue(undefined);
    render(<App />);

    expect(await screen.findByRole("heading", { name: "Welcome to Lore" })).toBeTruthy();
    const folders = screen.getByRole("group", { name: "Agent folders" });
    expect(within(folders).getByText("~/.claude/projects")).toBeTruthy();
    expect(within(folders).getByText("~/.codex/sessions")).toBeTruthy();
    expect(within(folders).getAllByText(/^(Found|Not found)$/).map((s) => s.textContent)).toEqual(["Found", "Not found"]);
    expect(screen.queryByRole("combobox", { name: "Search sessions" })).toBeNull();
    expect(within(screen.getByRole("navigation", { name: "Sidebar" })).queryAllByRole("button", { name: /All Sessions|New Folder/ })).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Add Folder…" }));
    await user.click(screen.getByRole("menuitem", { name: "Claude Code Folder…" }));
    await waitFor(() => expect(m.addAgentRoot).toHaveBeenCalledWith("claude-code", "/Volumes/old/claude"));

    await user.click(screen.getByRole("button", { name: "Start Scanning" }));
    expect(await screen.findByRole("button", { name: "Scanning…" })).toBeTruthy();
    expect(m.rescan).toHaveBeenCalledTimes(1);
  });

  it("shows a session that failed to open, with a retry", async () => {
    m.getSession.mockRejectedValueOnce(new Error("database is locked"));
    await renderLoaded();
    expect(await screen.findByText("Lore could not open this session.")).toBeTruthy();
    expect(screen.getByText("Error: database is locked")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));
    expect(await screen.findByRole("article", { name: "Fix flaky retry backoff" })).toBeTruthy();
  });
});

describe("reader", () => {
  function part(ordinal: number, kind: string, value: object | string): MessageDto["parts"][number] {
    return typeof value === "string"
      ? { ordinal, kind, text: value, content_json: JSON.stringify(value), searchable: true }
      : { ordinal, kind, text: null, content_json: JSON.stringify(value), searchable: false };
  }

  function withMessages(summary: SessionSummary, messages: MessageDto[]): SessionDetail {
    return { ...detail(summary), messages };
  }

  async function openAlpha() {
    await renderLoaded();
    return screen.findByRole("article", { name: "Fix flaky retry backoff" });
  }

  it("opens the newest session at launch with the selection in the list", async () => {
    await openAlpha();
    const list = screen.getByRole("listbox", { name: "Sessions in All Sessions" });
    expect(screen.getByRole("option", { name: /Fix flaky retry backoff/ }).getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(list);
  });

  it("says No Session Selected when nothing is open, with the actions disabled in place", async () => {
    m.listSessionsPage.mockResolvedValue({ sessions: [], next_cursor: "more" });
    render(<App />);
    expect(await screen.findByText("No Session Selected")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Copy as Markdown" }).hasAttribute("disabled")).toBe(true);
  });

  it("writes the session's facts in one line and keeps counts and cost in Details", async () => {
    const start = new Date();
    start.setHours(0, 5, 0, 0);
    const long: SessionSummary = {
      ...alpha,
      started_at: start.getTime(),
      ended_at: start.getTime() + 38 * 60_000,
      parse_status: "partial",
      total_input_tokens: 1000,
      total_output_tokens: 200,
      total_cache_tokens: 3000,
      est_cost_usd: 0.123,
    };
    m.getSession.mockResolvedValue({
      ...detail(long),
      parse_note: "skipped 2 records",
      segments: [{ id: "s1", seq_start: 0, seq_end: 1, cwd: "/repo", model: null, provider: null, repository_id: "repo-lore", resolution_confidence: "high" }],
    });
    m.sessionSecretCount.mockResolvedValue(2);
    const user = userEvent.setup();
    const reader = await openAlpha();

    expect(within(reader).getByText(/^Claude Code · claude-sonnet-4-5 · Today at .+ · 38 min · lore · Partially parsed · 2 secrets redacted$/)).toBeTruthy();
    await user.click(within(reader).getByRole("button", { name: "Details" }));
    const details = await screen.findByRole("dialog", { name: "Details" });
    expect(within(details).getAllByRole("definition").map((d) => d.textContent)).toEqual([
      "3, including 1 tool call",
      "1,000 in, 200 out, 3,000 cached",
      "$0.12, estimated",
    ]);
    expect(within(details).getByText(/^Lore read part of this session: skipped 2 records\./).textContent).toContain(
      "2 secrets redacted from search and exports.",
    );
  });

  it("switches tabs with ⌘1 to ⌘3 and counts files only", async () => {
    const reader = await openAlpha();
    expect(within(reader).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Conversation", "Files 1", "Git"]);

    fireEvent.keyDown(window, { key: "2", metaKey: true });
    expect(within(reader).getByRole("tabpanel").textContent).toContain("Edited 1 file,+1 −1");
    fireEvent.keyDown(window, { key: "3", metaKey: true });
    expect(within(reader).getByRole("tabpanel").textContent).toContain("No git evidence was recorded or observed for this session.");
    fireEvent.keyDown(window, { key: "1", metaKey: true });
    expect(within(reader).getByRole("tab", { name: "Conversation" }).getAttribute("aria-selected")).toBe("true");
  });

  it("shows a tool call as one line that opens to its recorded output and raw input", async () => {
    m.getSession.mockResolvedValue(
      withMessages(alpha, [
        message(1, "assistant", [part(0, "tool_use", { type: "tool_use", id: "t1", name: "Bash", input: { command: "cargo test sync", description: "Run the sync tests" } })]),
        message(2, "user", [part(0, "tool_result", "test sync::retry ... ok")]),
      ]),
    );
    const user = userEvent.setup();
    const reader = await openAlpha();
    const row = within(reader).getByRole("button", { name: "Ran cargo test sync" });
    expect(row.getAttribute("aria-expanded")).toBe("false");
    expect(within(reader).queryByText("test sync::retry ... ok")).toBeNull();

    await user.click(row);
    const output = within(reader).getByRole("figure", { name: "Output of Ran cargo test sync" });
    expect(within(output).getByText("test sync::retry ... ok")).toBeTruthy();
    expect(within(reader).getByRole("definition").textContent).toBe("Run the sync tests");

    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    try {
      fireEvent.click(within(output).getByRole("button", { name: "Copy Output" }));
      expect((await screen.findByRole("tooltip")).textContent).toBe("Copied");
      expect(writeText).toHaveBeenCalledWith("test sync::retry ... ok");
    } finally {
      Reflect.deleteProperty(navigator, "clipboard");
    }

    await user.click(within(reader).getByRole("button", { name: "Show Raw Input" }));
    expect(JSON.parse(within(reader).getByRole("figure", { name: "Raw input" }).textContent ?? "")).toEqual({
      command: "cargo test sync",
      description: "Run the sync tests",
    });
  });

  it("loads the next page of messages on request", async () => {
    m.getSession.mockResolvedValue({ ...detail(alpha), next_message_cursor: "c1" });
    m.listSessionMessagesPage.mockResolvedValue({
      messages: [message(3, "user", [part(0, "text", "One more thing about jitter.")])],
      next_cursor: null,
    });
    const reader = await openAlpha();
    fireEvent.click(within(reader).getByRole("button", { name: "Load More Messages" }));

    expect(await within(reader).findByText("One more thing about jitter.")).toBeTruthy();
    expect(m.listSessionMessagesPage).toHaveBeenCalledWith("a", 200, "c1");
    expect(within(reader).queryByRole("button", { name: "Load More Messages" })).toBeNull();
  });

  it("sums up a run of calls in one row that opens to each call", async () => {
    m.getSession.mockResolvedValue(
      withMessages(alpha, [
        message(1, "assistant", [
          part(0, "tool_use", { name: "Read", input: { file_path: "src/a.rs" } }),
          part(1, "tool_use", { name: "Bash", input: { command: "cargo build" } }),
        ]),
      ]),
    );
    const user = userEvent.setup();
    const reader = await openAlpha();
    expect(within(reader).queryByRole("button", { name: "Ran cargo build" })).toBeNull();

    await user.click(within(reader).getByRole("button", { name: "Read 1 file, ran 1 command" }));
    expect(within(reader).getByRole("button", { name: "Read src/a.rs" })).toBeTruthy();
    expect(within(reader).getByRole("button", { name: "Ran cargo build" })).toBeTruthy();
  });

  it("opens every call when Show Tool Calls is Expanded, and remembers it", async () => {
    m.getSession.mockResolvedValue(
      withMessages(alpha, [
        message(1, "assistant", [part(0, "tool_use", { name: "Bash", input: { command: "cargo test sync" } })]),
        message(2, "user", [part(0, "tool_result", "all tests passed")]),
      ]),
    );
    const user = userEvent.setup();
    const reader = await openAlpha();
    await user.click(screen.getByRole("button", { name: "More" }));
    await user.click(screen.getByRole("menuitemradio", { name: "Expanded" }));

    expect(within(reader).getByRole("button", { name: "Ran cargo test sync" }).getAttribute("aria-expanded")).toBe("true");
    expect(within(reader).getByRole("figure", { name: "Output of Ran cargo test sync" }).textContent).toContain("all tests passed");
    expect(m.setSetting).toHaveBeenCalledWith("ui.toolCalls", '"expanded"');
  });

  it("reads the stored tool-call preference at launch", async () => {
    m.getJsonSetting.mockImplementation(async (key: string) => (key === "ui.toolCalls" ? "expanded" : null));
    const reader = await openAlpha();
    await waitFor(() => expect(within(reader).getByRole("button", { name: "Ran cargo test sync" }).getAttribute("aria-expanded")).toBe("true"));
  });

  it("copies as Markdown from the toolbar or ⇧⌘C and confirms on the button", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    m.exportSessionMarkdown.mockResolvedValue("# Fix flaky retry backoff");
    try {
      await openAlpha();
      fireEvent.click(screen.getByRole("button", { name: "Copy as Markdown" }));
      expect((await screen.findByRole("tooltip")).textContent).toBe("Copied");
      expect(writeText).toHaveBeenCalledWith("# Fix flaky retry backoff");
      expect(m.exportSessionMarkdown).toHaveBeenCalledWith("a", false);

      fireEvent.keyDown(window, { key: "C", metaKey: true, shiftKey: true });
      await waitFor(() => expect(writeText).toHaveBeenCalledTimes(2));
    } finally {
      Reflect.deleteProperty(navigator, "clipboard");
    }
  });

  it("saves with ⌘S", async () => {
    m.saveSessionExport.mockResolvedValue(true);
    await openAlpha();
    fireEvent.keyDown(window, { key: "s", metaKey: true });
    expect(m.saveSessionExport).toHaveBeenCalledWith("a", null, false);
  });

  it("files the open session into a folder from the toolbar menu and confirms on the button", async () => {
    const user = userEvent.setup();
    await openAlpha();
    await user.click(screen.getByRole("button", { name: "File in Folder" }));
    await user.click(screen.getByRole("menuitem", { name: "Release prep" }));

    await waitFor(() => expect(m.setSessionFolder).toHaveBeenCalledWith("a", "f1"));
    expect((await screen.findByRole("tooltip")).textContent).toBe("Filed in “Release prep”");
  });

  it("forgets a session only after confirming, from the More menu or ⌘⌫", async () => {
    const user = userEvent.setup();
    await openAlpha();
    await user.click(screen.getByRole("button", { name: "More" }));
    await user.click(screen.getByRole("menuitem", { name: /^Forget Session…/ }));

    const dialog = screen.getByRole("alertdialog", { name: "Forget “Fix flaky retry backoff”?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(m.forgetSession).not.toHaveBeenCalled();
    expect(screen.getByRole("article", { name: "Fix flaky retry backoff" })).toBeTruthy();

    fireEvent.keyDown(document.body, { key: "Backspace", metaKey: true });
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Forget Session" }));
    await waitFor(() => expect(m.forgetSession).toHaveBeenCalledWith("a"));
    expect(await screen.findByText("No Session Selected")).toBeTruthy();
  });

  it("loads and renders a recorded patch with line numbers", async () => {
    const user = userEvent.setup();
    const reader = await openAlpha();
    await user.click(within(reader).getByRole("tab", { name: /Files/ }));
    await user.click(within(reader).getByRole("button", { name: "src/sync/worker.rs" }));

    const patch = await within(reader).findByRole("figure", { name: "Patch for src/sync/worker.rs" });
    const rows = within(patch).getAllByRole("row").map((row) => row.textContent);
    expect(rows).toEqual(["@@ -1,2 +1,2 @@", "1−let jitter = random();", "1+let jitter = self.rng.gen();", "22keep();"]);
    expect(m.getFilePatch).toHaveBeenCalledWith("fe1");
  });

  it("says which call changed a file and shows that call in the conversation", async () => {
    m.getSession.mockResolvedValue(
      withMessages(alpha, [
        message(1, "user", [part(0, "text", "Make it deterministic.")]),
        {
          ...message(2, "assistant", [part(0, "tool_use", { name: "Edit", input: { file_path: "/repo/src/sync/worker.rs", old_string: "a", new_string: "b" } })]),
          ts: new Date(2026, 9, 26, 14, 25).getTime(),
        },
      ]),
    );
    const user = userEvent.setup();
    const reader = await openAlpha();
    await user.click(within(reader).getByRole("tab", { name: /Files/ }));
    expect(within(reader).getByText(/^From an Edit call at 2:25/)).toBeTruthy();

    await user.click(within(reader).getByRole("button", { name: "Show in Conversation" }));
    expect(within(reader).getByRole("tab", { name: "Conversation" }).getAttribute("aria-selected")).toBe("true");
    const row = within(reader).getByRole("button", { name: "Edited /repo/src/sync/worker.rs +1 −1" });
    await waitFor(() => expect(document.activeElement).toBe(row));
  });

  it("states git evidence as sentences, newest first, naming each fact once", async () => {
    const start = new Date(2026, 9, 26, 14, 0).getTime();
    m.getSession.mockResolvedValue(detail({ ...alpha, started_at: start, ended_at: start + 30 * 60_000 }));
    m.getGitSnapshot.mockResolvedValue([
      {
        segment_id: null,
        source: "agent_recorded",
        event_ts: start,
        observed_at: start,
        temporal_confidence: "exact_event",
        branch: "main",
        commit_sha: null,
        remote_url_norm: "github.com/hsusul/lore",
        is_dirty: null,
        commit_exists: null,
      },
      {
        segment_id: null,
        source: "lore_captured",
        event_ts: null,
        observed_at: start + 32 * 60_000,
        temporal_confidence: "near_event",
        branch: "main",
        commit_sha: "abc1234def",
        remote_url_norm: "github.com/hsusul/lore",
        is_dirty: true,
        commit_exists: null,
      },
    ]);
    const user = userEvent.setup();
    const reader = await openAlpha();
    await user.click(within(reader).getByRole("tab", { name: "Git" }));
    const list = within(reader).getByRole("list", { name: "Git observations" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "Commit abc1234 captured by Lore 2 minutes after the session ended. The working tree had uncommitted changes.2:32 PM",
      "Branch main on github.com/hsusul/lore recorded by Claude Code when the session started.2:00 PM",
    ]);
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

    const search = screen.getByRole("search");
    expect((within(search).getByRole("combobox", { name: "Search sessions" }) as HTMLInputElement).value).toBe("");
    expect(search.textContent).toBe("agent:claude-code");
    await waitFor(() => expect(m.searchPage).toHaveBeenLastCalledWith("agent:claude-code", 50, null, "relevance"));
    expect(within(sidebar()).getByRole("button", { name: /Claude Code/ }).getAttribute("aria-current")).toBe("true");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Claude Code");

    fireEvent.click(within(sidebar()).getByRole("button", { name: /All Sessions/ }));
    expect((screen.getByRole("combobox", { name: "Search sessions" }) as HTMLInputElement).value).toBe("");
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
    const input = await screen.findByRole("combobox", { name: "Search sessions and actions" });
    fireEvent.change(input, { target: { value: "Delete “Release prep”" } });
    fireEvent.keyDown(input, { key: "Enter" });
    m.listFolders.mockResolvedValue([]);
    fireEvent.click(within(screen.getByRole("alertdialog", { name: "Delete “Release prep”?" })).getByRole("button", { name: "Delete Folder" }));
    await waitFor(() => expect(m.deleteFolder).toHaveBeenCalledWith("f1"));
    await waitFor(() => expect(within(sidebar()).queryByRole("button", { name: /Release prep/ })).toBeNull());
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("creates a folder from the sidebar, and it appears there", async () => {
    m.createFolder.mockResolvedValue({ id: "f2", name: "Ideas", session_count: 0, position: 1 });
    await renderLoaded();
    fireEvent.click(within(sidebar()).getByRole("button", { name: "New Folder" }));
    const dialog = screen.getByRole("dialog", { name: "New Folder" });
    expect(within(dialog).getByRole("button", { name: "Create" }).hasAttribute("disabled")).toBe(true);
    m.listFolders.mockResolvedValue([
      { id: "f1", name: "Release prep", session_count: 0, position: 0 },
      { id: "f2", name: "Ideas", session_count: 0, position: 1 },
    ]);
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name:" }), { target: { value: "Ideas" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(m.createFolder).toHaveBeenCalledWith("Ideas"));
    expect(await within(sidebar()).findByRole("button", { name: /Ideas/ })).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: "New Folder" })).toBeNull();
  });

  it("keeps the New Folder sheet open with the error when creating fails, and has no notices line", async () => {
    m.createFolder.mockRejectedValue("a folder named Ideas already exists");
    await renderLoaded();
    fireEvent.click(within(sidebar()).getByRole("button", { name: "New Folder" }));
    const dialog = screen.getByRole("dialog", { name: "New Folder" });
    fireEvent.change(within(dialog).getByRole("textbox"), { target: { value: "Ideas" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    expect((await within(dialog).findByRole("alert")).textContent).toBe("a folder named Ideas already exists");
    expect(screen.getByRole("dialog", { name: "New Folder" })).toBe(dialog);
    expect(screen.queryByRole("button", { name: "Dismiss" })).toBeNull();
  });

  it("reaches Settings by mouse through the palette button", async () => {
    await renderLoaded();
    fireEvent.click(screen.getByRole("button", { name: "Command Palette" }));
    const input = await screen.findByRole("combobox", { name: "Search sessions and actions" });
    fireEvent.change(input, { target: { value: "Settings" } });
    fireEvent.click(screen.getByRole("option", { name: /^Settings/ }));
    expect(await screen.findByRole("dialog", { name: /Settings/ })).toBeTruthy();
  });

  it("says on the Rescan control when reloading the archive fails, and tries again from there", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    await screen.findByRole("article", { name: "Fix flaky retry backoff" });
    m.listFolders.mockRejectedValueOnce("database is locked");
    m.setSessionFolder.mockResolvedValue(undefined);
    await user.click(screen.getByRole("button", { name: "File in Folder" }));
    await user.click(screen.getByRole("menuitem", { name: "Release prep" }));

    await user.click(await screen.findByRole("button", { name: "Archive not updated" }));
    const popover = screen.getByRole("dialog", { name: "Archive not updated" });
    expect(within(popover).getByText("Lore could not reload the archive.")).toBeTruthy();
    expect(within(popover).getByText("database is locked")).toBeTruthy();
    const loads = m.listFolders.mock.calls.length;
    await user.click(within(popover).getByRole("button", { name: "Try Again" }));
    await waitFor(() => expect(m.listFolders.mock.calls.length).toBe(loads + 1));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Archive not updated" })).toBeNull());
  });
});

describe("settings", () => {
  async function openSettings() {
    await renderLoaded();
    fireEvent.keyDown(window, { key: ",", metaKey: true });
    return screen.findByRole("dialog", { name: "Settings" });
  }

  it("opens with ⌘, on General and moves between panes by click and arrow keys", async () => {
    const user = userEvent.setup();
    const settings = await openSettings();
    expect(within(settings).getAllByRole("tab").map((t) => t.textContent)).toEqual(["General", "Sources", "Search", "Backups", "Privacy"]);
    expect(within(settings).getByRole("tabpanel", { name: "General" })).toBeTruthy();

    await user.click(within(settings).getByRole("tab", { name: "Backups" }));
    expect(within(settings).getByRole("heading", { name: "Backups" })).toBeTruthy();
    fireEvent.keyDown(within(settings).getByRole("tab", { name: "Backups" }), { key: "ArrowDown" });
    expect(within(settings).getByRole("tabpanel", { name: "Privacy" })).toBeTruthy();
    expect(document.activeElement).toBe(within(settings).getByRole("tab", { name: "Privacy" }));

    await user.click(within(settings).getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
  });

  it("sets the appearance and density on the window, and remembers them", async () => {
    const user = userEvent.setup();
    const settings = await openSettings();
    await user.click(within(settings).getByRole("combobox", { name: "Appearance" }));
    await user.click(screen.getByRole("option", { name: "Dark" }));
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(m.setSetting).toHaveBeenCalledWith("theme", '"dark"');

    await user.click(within(settings).getByRole("combobox", { name: "Density" }));
    await user.click(screen.getByRole("option", { name: "Compact" }));
    expect(document.documentElement.dataset.density).toBe("compact");
    expect(m.setSetting).toHaveBeenCalledWith("ui.density", '"compact"');

    await user.click(within(settings).getByRole("combobox", { name: "Appearance" }));
    await user.click(screen.getByRole("option", { name: "System" }));
    expect(document.documentElement.dataset.theme).toBeUndefined();
  });

  it("applies the stored appearance and density at launch", async () => {
    m.getJsonSetting.mockImplementation(async (key: string) => (key === "theme" ? "light" : key === "ui.density" ? "spacious" : null));
    await renderLoaded();
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
    expect(document.documentElement.dataset.density).toBe("spacious");
  });

  it("adds and removes agent folders in Sources and says so in place", async () => {
    const user = userEvent.setup();
    m.listDetectedAgents.mockResolvedValue([
      { ...agents[0], roots: ["~/.claude/projects", "/Volumes/old/claude"], custom_roots: ["/Volumes/old/claude"] },
    ]);
    m.chooseAgentRootDirectory.mockResolvedValue("/Volumes/new/claude");
    m.addAgentRoot.mockResolvedValue(undefined);
    m.removeAgentRoot.mockResolvedValue(undefined);
    const settings = await openSettings();
    await user.click(within(settings).getByRole("tab", { name: "Sources" }));
    const claude = within(settings).getByRole("region", { name: "Claude Code" });
    expect(within(claude).getByText("Default")).toBeTruthy();
    expect(within(claude).getByText("Added by you")).toBeTruthy();

    await user.click(within(claude).getByRole("button", { name: "Add a Claude Code Folder…" }));
    expect((await within(claude).findByRole("status")).textContent).toBe("Lore is scanning /Volumes/new/claude now.");
    expect(m.addAgentRoot).toHaveBeenCalledWith("claude-code", "/Volumes/new/claude");

    await user.click(within(claude).getByRole("button", { name: "Remove /Volumes/old/claude from Claude Code" }));
    await waitFor(() =>
      expect(within(claude).getByRole("status").textContent).toBe("Lore no longer reads /Volumes/old/claude. Sessions already archived stay."),
    );
    expect(m.removeAgentRoot).toHaveBeenCalledWith("claude-code", "/Volumes/old/claude");
  });

  it("schedules backups and backs up now from Backups", async () => {
    const user = userEvent.setup();
    m.setBackupSchedule.mockResolvedValue(undefined);
    m.backupNow.mockResolvedValue(undefined);
    const settings = await openSettings();
    await user.click(within(settings).getByRole("tab", { name: "Backups" }));
    const keep = within(settings).getByRole("spinbutton", { name: "Copies to keep" });
    expect(keep.hasAttribute("disabled")).toBe(true);

    await user.click(within(settings).getByRole("combobox", { name: "Automatic backups" }));
    await user.click(screen.getByRole("option", { name: "Weekly" }));
    expect(m.setBackupSchedule).toHaveBeenCalledWith("weekly", 7);
    expect(keep.hasAttribute("disabled")).toBe(false);

    await user.click(within(settings).getByRole("button", { name: "Back Up Now" }));
    expect((await within(settings).findByRole("status")).textContent).toBe("Backup created.");
  });

  it("sets the search order from the Search pane", async () => {
    const user = userEvent.setup();
    const settings = await openSettings();
    await user.click(within(settings).getByRole("tab", { name: "Search" }));
    await user.click(within(settings).getByRole("combobox", { name: "Sort results by" }));
    await user.click(screen.getByRole("option", { name: "Oldest" }));
    await user.click(within(settings).getByRole("button", { name: "Done" }));
    typeQuery("retry");
    expect(await screen.findByRole("button", { name: "Sort results, Oldest" })).toBeTruthy();
  });

  it("forgets everything only after an alert, and says what it removed", async () => {
    const user = userEvent.setup();
    m.forgetEverything.mockResolvedValue({ blobs_removed: 12, source_paths: [] });
    const settings = await openSettings();
    await user.click(within(settings).getByRole("tab", { name: "Privacy" }));
    expect(within(settings).getByText(/The archive stays on this Mac/)).toBeTruthy();

    await user.click(within(settings).getByRole("button", { name: "Forget Everything…" }));
    const alert = screen.getByRole("alertdialog", { name: "Forget everything in Lore?" });
    expect(document.activeElement).toBe(within(alert).getByRole("button", { name: "Cancel" }));
    await user.click(within(alert).getByRole("button", { name: "Cancel" }));
    expect(m.forgetEverything).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Settings" })).toBe(settings);

    m.listDetectedAgents.mockResolvedValue([{ ...agents[0], session_count: 0 }]);
    m.listRepositories.mockResolvedValue([]);
    m.listSessionsPage.mockResolvedValue({ sessions: [], next_cursor: null });
    await user.click(within(settings).getByRole("button", { name: "Forget Everything…" }));
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Forget Everything" }));
    expect((await within(settings).findByRole("status")).textContent).toBe("Lore forgot everything and removed 12 stored files.");
    await user.click(within(settings).getByRole("button", { name: "Done" }));
    expect(await screen.findByRole("heading", { name: "Welcome to Lore" })).toBeTruthy();
  });
});

describe("alerts and menus", () => {
  it("starts an alert on Cancel, keeps Tab inside it, and keeps it open with the error when forgetting fails", async () => {
    const user = userEvent.setup();
    m.forgetSession.mockRejectedValueOnce("the archive is read-only");
    await renderLoaded();
    await screen.findByRole("article", { name: "Fix flaky retry backoff" });
    fireEvent.keyDown(document.body, { key: "Backspace", metaKey: true });

    const alert = screen.getByRole("alertdialog", { name: "Forget “Fix flaky retry backoff”?" });
    expect(within(alert).getByText(/The original agent log on this Mac is not changed\./)).toBeTruthy();
    const cancel = within(alert).getByRole("button", { name: "Cancel" });
    const forget = within(alert).getByRole("button", { name: "Forget Session" });
    expect(document.activeElement).toBe(cancel);
    await user.tab();
    expect(document.activeElement).toBe(forget);
    await user.tab();
    expect(document.activeElement).toBe(cancel);

    await user.click(forget);
    expect((await within(alert).findByRole("alert")).textContent).toBe("the archive is read-only");
    expect(screen.getByRole("alertdialog")).toBe(alert);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByRole("article", { name: "Fix flaky retry backoff" })).toBeTruthy();
  });

  it("acts on a right-clicked session row without opening it", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    m.exportSessionMarkdown.mockResolvedValue("# Port dotfiles to nix");
    try {
      await renderLoaded();
      await screen.findByRole("article", { name: "Fix flaky retry backoff" });
      const row = screen.getByRole("option", { name: /Port dotfiles to nix/ });

      await user.pointer({ keys: "[MouseRight]", target: row });
      expect(screen.getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
        "Copy as Markdown",
        "Save as File…",
        "File in Folder",
        "Forget Session…",
      ]);
      await user.click(screen.getByRole("menuitem", { name: "Copy as Markdown" }));
      await waitFor(() => expect(writeText).toHaveBeenCalledWith("# Port dotfiles to nix"));
      expect(m.exportSessionMarkdown).toHaveBeenCalledWith("b", false);

      await user.pointer({ keys: "[MouseRight]", target: row });
      await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{ArrowRight}");
      expect(document.activeElement?.textContent).toBe("Release prep");
      await user.keyboard("{Enter}");
      await waitFor(() => expect(m.setSessionFolder).toHaveBeenCalledWith("b", "f1"));
      expect(row.getAttribute("aria-selected")).toBe("false");

      await user.pointer({ keys: "[MouseRight]", target: row });
      await user.click(screen.getByRole("menuitem", { name: "Forget Session…" }));
      await user.click(within(screen.getByRole("alertdialog", { name: "Forget “Port dotfiles to nix”?" })).getByRole("button", { name: "Forget Session" }));
      await waitFor(() => expect(m.forgetSession).toHaveBeenCalledWith("b"));
      await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
      expect(screen.getByRole("article", { name: "Fix flaky retry backoff" })).toBeTruthy();
    } finally {
      Reflect.deleteProperty(navigator, "clipboard");
    }
  });
});

describe("palette", () => {
  async function openPalette() {
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    return screen.findByRole("combobox", { name: "Search sessions and actions" });
  }
  const rows = (group: string) =>
    within(screen.getByRole("group", { name: group }))
      .getAllByRole("option")
      .map((o) => o.textContent);

  it("lists every action with its shortcut in Apple glyphs, then the view choices and places", async () => {
    await renderLoaded();
    await screen.findByRole("article", { name: "Fix flaky retry backoff" });
    await openPalette();
    expect(rows("Actions")).toEqual([
      "Copy as Markdown⇧⌘C",
      "Save as File…⌘S",
      "File in “Release prep”",
      "Remove from Folder",
      "Forget Session…⌘⌫",
      "Rescan⌘R",
      "Search Sessions⌥⌘F",
      "Filter Reference⌘/",
      "New Folder…",
      "Settings…⌘,",
    ]);
    expect(rows("View")).toEqual([
      "Show Conversation⌘1",
      "Show Files⌘2",
      "Show Git⌘3",
      "Show All Sessions",
      "Hide Sidebar⌃⌘S",
      "Sort by Newest",
      "Sort by Oldest",
      "Use Light Appearance",
      "Use Dark Appearance",
      "Use Compact Density",
      "Use Spacious Density",
      "Show Tool Calls Expanded",
    ]);
    expect(rows("Sessions")).toEqual(["Fix flaky retry backoff", "Port dotfiles to nix"]);
    expect(rows("Repositories")).toEqual(["lore1 session"]);
    expect(rows("Folders")).toEqual(["Release prep0 sessions"]);
  });

  it("changes the appearance, the reader tab, the sort and the scope, and opens the filter reference", async () => {
    const user = userEvent.setup();
    await renderLoaded();
    await screen.findByRole("article", { name: "Fix flaky retry backoff" });

    await user.type(await openPalette(), "dark");
    await user.keyboard("{Enter}");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(m.setSetting).toHaveBeenCalledWith("theme", '"dark"');

    await user.type(await openPalette(), "show files");
    await user.keyboard("{Enter}");
    expect(screen.getByRole("tab", { name: /^Files/ }).getAttribute("aria-selected")).toBe("true");

    await user.type(await openPalette(), "filter reference");
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("dialog", { name: "Search filters" })).toBeTruthy();
    await user.keyboard("{Escape}");

    await user.type(await openPalette(), "lore");
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("heading", { name: "lore", level: 1 })).toBeTruthy();

    await user.type(await openPalette(), "newest");
    await user.keyboard("{Enter}");
    typeQuery("retry");
    expect(await screen.findByRole("button", { name: "Sort results, Newest" })).toBeTruthy();
  });
});

describe("shortcuts", () => {
  it("opens the filter reference with ⌘/ and ignores window shortcuts while a sheet is open", async () => {
    await renderLoaded();
    fireEvent.keyDown(window, { key: "/", metaKey: true });
    expect(await screen.findByRole("dialog", { name: "Search filters" })).toBeTruthy();
    fireEvent.keyDown(window, { key: "/", metaKey: true });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Search filters" })).toBeNull());

    fireEvent.keyDown(window, { key: ",", metaKey: true });
    const settings = await screen.findByRole("dialog", { name: "Settings" });
    fireEvent.keyDown(window, { key: "r", metaKey: true });
    expect(m.rescan).toHaveBeenCalledTimes(0);
    fireEvent.click(within(settings).getByRole("button", { name: "Done" }));
    fireEvent.keyDown(window, { key: "r", metaKey: true });
    await waitFor(() => expect(m.rescan).toHaveBeenCalledTimes(1));
  });
});
