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

    // Arrows move the selection and open each session in place; Enter moves into the reader.
    fireEvent.keyDown(list, { key: "ArrowDown" });
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
    expect(toolbar()).toEqual(["Rescan", "Command Palette", "Search Filters"]);
    fireEvent.click(screen.getByRole("button", { name: "Rescan" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Scan failed: background ingestion worker unavailable");
    expect(toolbar()).toEqual(["Scan failed", "Rescan", "Command Palette", "Search Filters"]);

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
