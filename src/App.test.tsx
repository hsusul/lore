import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const scan = vi.hoisted(() => ({
  handler: null as null | ((progress: {
    discovered: number;
    ingested: number;
    skipped: number;
    failed: number;
    enriched: number;
    done: boolean;
  }) => void),
}));

vi.mock("./ipc", () => ({
  listDetectedAgents: vi.fn().mockResolvedValue([]),
  chooseAgentRootDirectory: vi.fn().mockResolvedValue(null),
  addAgentRoot: vi.fn().mockResolvedValue(undefined),
  removeAgentRoot: vi.fn().mockResolvedValue(undefined),
  listRepositories: vi.fn().mockResolvedValue([]),
  listSessionsPage: vi.fn().mockResolvedValue({ sessions: [], next_cursor: null }),
  listRepositorySessionsPage: vi.fn().mockResolvedValue({
    sessions: [],
    next_cursor: null,
  }),
  listFolders: vi.fn().mockResolvedValue([]),
  listFolderSessionsPage: vi.fn().mockResolvedValue({ sessions: [], next_cursor: null }),
  createFolder: vi.fn().mockResolvedValue({ id: "f", name: "f", session_count: 0, position: 0 }),
  renameFolder: vi.fn().mockResolvedValue(undefined),
  deleteFolder: vi.fn().mockResolvedValue(undefined),
  setSessionFolder: vi.fn().mockResolvedValue(undefined),
  getSession: vi.fn().mockResolvedValue(null),
  getGitSnapshot: vi.fn().mockResolvedValue([]),
  getFilePatch: vi.fn().mockResolvedValue(null),
  sessionSecretCount: vi.fn().mockResolvedValue(0),
  exportSessionMarkdown: vi.fn().mockResolvedValue(""),
  forgetSession: vi.fn().mockResolvedValue({ blobs_removed: 0, source_paths: [] }),
  forgetEverything: vi.fn().mockResolvedValue({ blobs_removed: 0, source_paths: [] }),
  search: vi.fn().mockResolvedValue([]),
  searchPage: vi.fn().mockResolvedValue({ hits: [], next_cursor: null }),
  getSetting: vi.fn().mockResolvedValue(null),
  setSetting: vi.fn().mockResolvedValue(undefined),
  getBackupSchedule: vi.fn().mockResolvedValue({ interval: "off", keep: 5 }),
  setBackupSchedule: vi.fn().mockResolvedValue(undefined),
  backupNow: vi.fn().mockResolvedValue(undefined),
  rescan: vi.fn().mockResolvedValue({}),
  onScanProgress: vi.fn().mockImplementation(async (handler) => {
    scan.handler = handler;
    return () => {};
  }),
  HIGHLIGHT_START: "\u{e000}",
  HIGHLIGHT_END: "\u{e001}",
}));

import App from "./App";
import {
  addAgentRoot,
  chooseAgentRootDirectory,
  forgetEverything,
  getSession,
  listDetectedAgents,
  listFolders,
  listRepositories,
  listRepositorySessionsPage,
  listSessionsPage,
  rescan,
  removeAgentRoot,
  searchPage,
  type DetectedAgent,
  type RepositorySummary,
  type SessionPage,
  type SessionDetail,
  type SessionSummary,
} from "./ipc";

function summary(id: string, title: string): SessionSummary {
  return {
    id,
    agent_id: "codex",
    title,
    started_at: null,
    ended_at: null,
    message_count: 0,
    tool_call_count: 0,
    primary_model: null,
    parse_status: "ok",
  };
}

function detail(session: SessionSummary): SessionDetail {
  return { summary: session, parse_note: null, segments: [], messages: [], file_events: [] };
}

function repository(id: string, name: string): RepositorySummary {
  return {
    id,
    display_name: name,
    identity_confidence: "confirmed",
    primary_path: null,
    is_missing: false,
    session_count: 1,
    worktree_count: 1,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

afterEach(() => {
  vi.mocked(listDetectedAgents).mockResolvedValue([]);
  vi.mocked(chooseAgentRootDirectory).mockResolvedValue(null);
  vi.mocked(addAgentRoot).mockResolvedValue(undefined);
  vi.mocked(removeAgentRoot).mockResolvedValue(undefined);
  vi.mocked(listRepositories).mockResolvedValue([]);
  vi.mocked(listSessionsPage).mockResolvedValue({ sessions: [], next_cursor: null });
  vi.mocked(listRepositorySessionsPage).mockResolvedValue({
    sessions: [],
    next_cursor: null,
  });
  vi.mocked(getSession).mockResolvedValue(null);
  vi.mocked(rescan).mockClear();
  vi.mocked(searchPage).mockResolvedValue({ hits: [], next_cursor: null });
});

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

  it("shows loading rather than a false empty archive while initial queries settle", async () => {
    const agents = deferred<DetectedAgent[]>();
    vi.mocked(listDetectedAgents).mockReturnValueOnce(agents.promise);

    render(<App />);

    expect(screen.queryByRole("heading", { name: /your agent history/i })).toBeNull();
    expect(screen.getByText("Loading archive…")).toBeTruthy();

    await act(async () => agents.resolve([]));
    expect(
      await screen.findByRole("heading", { name: "Your agent history, searchable." }),
    ).toBeTruthy();
  });

  it("offers a truthful local-first onboarding path for an empty archive", async () => {
    vi.mocked(listDetectedAgents).mockResolvedValue([
      {
        id: "claude-code",
        display_name: "Claude Code",
        installed: true,
        version: null,
        session_count: 0,
        roots: ["/Users/dev/.claude/projects"],
        custom_roots: [],
      },
      {
        id: "codex",
        display_name: "Codex",
        installed: false,
        version: null,
        session_count: 0,
        roots: ["/Users/dev/.codex/sessions"],
        custom_roots: [],
      },
    ]);

    render(<App />);

    expect(
      await screen.findByRole("heading", { name: "Your agent history, searchable." }),
    ).toBeTruthy();
    expect(screen.getByText("/Users/dev/.claude/projects")).toBeTruthy();
    expect(screen.getByText("/Users/dev/.codex/sessions")).toBeTruthy();
    expect(screen.getByText(/local and read-only/i)).toBeTruthy();
    expect(screen.getByText("Claude Code").closest("li")?.textContent).toContain("detected");
    expect(screen.getByText("Codex").closest("li")?.textContent).toContain("not detected");

    vi.mocked(chooseAgentRootDirectory).mockResolvedValueOnce("/Volumes/archive/codex");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Choose Codex folder" }));
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(chooseAgentRootDirectory).toHaveBeenCalledWith("Codex");
    expect(addAgentRoot).toHaveBeenCalledWith("codex", "/Volumes/archive/codex");
    expect(screen.getByRole("status").textContent).toContain("Codex folder added");

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Scan agent history" }));
      await Promise.resolve();
    });
    expect(rescan).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Scanning agent history…" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Review privacy and agents" }));
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeTruthy();
  });

  it("never relabels a failed initial load as an empty archive", async () => {
    vi.mocked(listDetectedAgents).mockRejectedValueOnce(new Error("archive unavailable"));

    render(<App />);

    expect((await screen.findByRole("alert")).textContent).toContain("archive unavailable");
    expect(screen.getByText("Sessions unavailable.")).toBeTruthy();
    expect(screen.queryByText("No sessions yet.")).toBeNull();
    fireEvent.change(screen.getByRole("searchbox", { name: "search" }), {
      target: { value: "retry" },
    });

    expect(screen.queryByRole("heading", { name: /your agent history/i })).toBeNull();
    expect(screen.getByText("Archive unavailable.")).toBeTruthy();
  });

  it("opens the command palette on ⌘K", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Lore", level: 1 });
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(screen.getByRole("dialog", { name: /command palette/i })).toBeTruthy();
  });

  it("returns focus to the palette trigger after Escape", async () => {
    render(<App />);
    const trigger = await screen.findByRole("button", { name: /open command palette/i });
    trigger.focus();
    fireEvent.click(trigger);
    const input = screen.getByRole("combobox");
    expect(document.activeElement).toBe(input);

    fireEvent.keyDown(input, { key: "Escape" });

    expect(screen.queryByRole("dialog", { name: /command palette/i })).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("finds and opens an archived session from the command palette", async () => {
    const archived = summary("archive-session", "Retry with exponential backoff");
    vi.mocked(searchPage).mockResolvedValue({
      hits: [
        {
          session_id: archived.id,
          source_kind: "message_part",
          source_id: "part-1",
          field: "text",
          snippet: "use exponential backoff",
          rank: -1,
          title: archived.title,
          agent_id: archived.agent_id,
          started_at: archived.started_at,
        },
      ],
      next_cursor: null,
    });
    vi.mocked(getSession).mockResolvedValue(detail(archived));
    vi.useFakeTimers();

    try {
      render(<App />);
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      fireEvent.keyDown(window, { key: "k", metaKey: true });
      const palette = screen.getByRole("combobox");
      const searchPageMock = vi.mocked(searchPage);
      searchPageMock.mockClear();

      fireEvent.change(palette, { target: { value: "backoff" } });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(200);
      });

      expect(searchPageMock).toHaveBeenCalledWith("backoff", 50);
      expect(screen.getByText("Retry with exponential backoff")).toBeTruthy();
      await act(async () => {
        fireEvent.keyDown(palette, { key: "Enter" });
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(getSession).toHaveBeenCalledWith(archived.id);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps rescan busy until the worker reports completion", async () => {
    render(<App />);
    const button = await screen.findByRole("button", { name: "Rescan" });
    fireEvent.click(button);
    expect(await screen.findByRole("button", { name: "Scanning…" })).toBeTruthy();

    act(() => {
      scan.handler?.({
        discovered: 2,
        ingested: 1,
        skipped: 0,
        failed: 0,
        enriched: 1,
        done: true,
      });
    });
    expect(await screen.findByRole("button", { name: "Rescan" })).toBeTruthy();
  });

  it("debounces rapid search input and only queries the latest text", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Lore", level: 1 });

    const search = screen.getByRole("searchbox", { name: "search" });
    const searchPageMock = vi.mocked(searchPage);
    searchPageMock.mockClear();
    vi.useFakeTimers();

    try {
      fireEvent.change(search, { target: { value: "r" } });
      fireEvent.change(search, { target: { value: "ret" } });
      fireEvent.change(search, { target: { value: "retry" } });

      expect(searchPageMock).not.toHaveBeenCalled();

      await act(async () => {
        vi.advanceTimersByTime(200);
        await Promise.resolve();
      });

      expect(searchPageMock).toHaveBeenCalledTimes(1);
      expect(searchPageMock).toHaveBeenCalledWith("retry", 50);
    } finally {
      vi.useRealTimers();
    }
  });

  it("moves keyboard focus from search into its results and back", async () => {
    vi.mocked(searchPage).mockResolvedValue({
      hits: [
        {
          session_id: "search-session",
          source_kind: "message_part",
          source_id: "search-part",
          field: "text",
          snippet: "retry result",
          rank: -1,
          title: "Retry result",
          agent_id: "codex",
          started_at: null,
        },
      ],
      next_cursor: null,
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Lore", level: 1 });
    const search = screen.getByRole("searchbox", { name: "search" });
    vi.useFakeTimers();

    try {
      fireEvent.change(search, { target: { value: "retry" } });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(200);
      });
      const results = screen.getByRole("listbox", { name: /search results/i });

      search.focus();
      fireEvent.keyDown(search, { key: "ArrowDown" });
      expect(document.activeElement).toBe(results);

      fireEvent.keyDown(results, { key: "ArrowUp" });
      expect(document.activeElement).toBe(search);
    } finally {
      vi.useRealTimers();
    }
  });

  it("removes results from the previous query as soon as new text is entered", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Lore", level: 1 });

    const search = screen.getByRole("searchbox", { name: "search" });
    const searchPageMock = vi.mocked(searchPage);
    searchPageMock.mockClear();
    searchPageMock.mockResolvedValueOnce({
      hits: [
        {
          session_id: "s1",
          source_kind: "message_part",
          source_id: "p1",
          field: "text",
          snippet: "alpha result",
          rank: -1,
          title: "Old alpha result",
          agent_id: "codex",
          started_at: null,
        },
      ],
      next_cursor: null,
    });
    vi.useFakeTimers();

    try {
      fireEvent.change(search, { target: { value: "alpha" } });
      await act(async () => {
        vi.advanceTimersByTime(200);
        await Promise.resolve();
      });
      expect(screen.getByText("Old alpha result")).toBeTruthy();

      fireEvent.change(search, { target: { value: "beta" } });
      expect(screen.queryByText("Old alpha result")).toBeNull();
      expect(screen.getByRole("status").textContent).toContain("Searching");
    } finally {
      vi.useRealTimers();
      searchPageMock.mockResolvedValue({ hits: [], next_cursor: null });
    }
  });

  it("cancels a pending search when the query is cleared", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Lore", level: 1 });

    const search = screen.getByRole("searchbox", { name: "search" });
    const searchPageMock = vi.mocked(searchPage);
    searchPageMock.mockClear();
    vi.useFakeTimers();

    try {
      fireEvent.change(search, { target: { value: "retry" } });
      fireEvent.change(search, { target: { value: "" } });

      await act(async () => {
        vi.advanceTimersByTime(200);
        await Promise.resolve();
      });

      expect(searchPageMock).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("loads a newly selected repository only once", async () => {
    vi.mocked(listRepositories).mockResolvedValue([repository("repo-a", "Repo A")]);
    const repositorySessions = vi.mocked(listRepositorySessionsPage);
    repositorySessions.mockResolvedValue({ sessions: [], next_cursor: null });

    render(<App />);
    const repoButton = await screen.findByRole("button", { name: /Repo A/i });
    repositorySessions.mockClear();

    fireEvent.click(repoButton);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(repositorySessions).toHaveBeenCalledTimes(1);
    expect(repositorySessions).toHaveBeenCalledWith("repo-a", 100, null);
  });

  it("keeps sessions for the latest repository when responses finish out of order", async () => {
    vi.mocked(listRepositories).mockResolvedValue([
      repository("repo-a", "Repo A"),
      repository("repo-b", "Repo B"),
    ]);
    const repoA = deferred<SessionPage>();
    const repoB = deferred<SessionPage>();
    vi.mocked(listRepositorySessionsPage).mockImplementation((id) =>
      id === "repo-a" ? repoA.promise : repoB.promise,
    );

    render(<App />);
    const repoAButton = await screen.findByRole("button", { name: /Repo A/i });
    const repoBButton = screen.getByRole("button", { name: /Repo B/i });

    fireEvent.click(repoAButton);
    fireEvent.click(repoBButton);
    expect(screen.getByRole("status").textContent).toContain("Loading sessions");
    await act(async () =>
      repoB.resolve({
        sessions: [summary("session-b", "B session")],
        next_cursor: null,
      }),
    );
    expect(screen.getByText("B session")).toBeTruthy();

    await act(async () =>
      repoA.resolve({
        sessions: [summary("session-a", "A session")],
        next_cursor: null,
      }),
    );
    expect(screen.queryByText("A session")).toBeNull();
    expect(screen.getByText("B session")).toBeTruthy();
  });

  it("ignores an error from a superseded repository request", async () => {
    vi.mocked(listRepositories).mockResolvedValue([
      repository("repo-a", "Repo A"),
      repository("repo-b", "Repo B"),
    ]);
    const repoA = deferred<SessionPage>();
    const repoB = deferred<SessionPage>();
    vi.mocked(listRepositorySessionsPage).mockImplementation((id) =>
      id === "repo-a" ? repoA.promise : repoB.promise,
    );

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /Repo A/i }));
    fireEvent.click(screen.getByRole("button", { name: /Repo B/i }));
    await act(async () =>
      repoB.resolve({
        sessions: [summary("session-b", "B session")],
        next_cursor: null,
      }),
    );
    await act(async () => repoA.reject(new Error("stale repository failure")));

    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByText("B session")).toBeTruthy();
  });

  it("settles repository loading when a background refresh supersedes it", async () => {
    vi.mocked(listRepositories).mockResolvedValue([repository("repo-a", "Repo A")]);
    const manual = deferred<SessionPage>();
    const repositorySessions = vi.mocked(listRepositorySessionsPage);
    repositorySessions
      .mockImplementationOnce(() => manual.promise)
      .mockResolvedValue({
        sessions: [summary("refreshed", "Refreshed session")],
        next_cursor: null,
      });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /Repo A/i }));
    expect(screen.getByRole("status").textContent).toContain("Loading sessions");

    vi.useFakeTimers();
    try {
      act(() => {
        scan.handler?.({
          discovered: 1,
          ingested: 1,
          skipped: 0,
          failed: 0,
          enriched: 1,
          done: true,
        });
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(300);
      });

      expect(screen.queryByText("Loading sessions…")).toBeNull();
      expect(screen.getByText("Refreshed session")).toBeTruthy();

      await act(async () =>
        manual.resolve({
          sessions: [summary("stale", "Stale session")],
          next_cursor: null,
        }),
      );
      expect(screen.queryByText("Stale session")).toBeNull();
      expect(screen.getByText("Refreshed session")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("starts with a bounded session page and appends older sessions", async () => {
    const pages = vi.mocked(listSessionsPage);
    pages.mockClear();
    pages
      .mockResolvedValueOnce({
        sessions: [summary("new-session", "New session")],
        next_cursor: "older-cursor",
      })
      .mockResolvedValueOnce({
        sessions: [summary("old-session", "Old session")],
        next_cursor: null,
      });

    render(<App />);
    expect(await screen.findByText("New session")).toBeTruthy();
    expect(pages).toHaveBeenNthCalledWith(1, 100, null);

    fireEvent.click(screen.getByRole("button", { name: /load older sessions/i }));
    expect(await screen.findByText("Old session")).toBeTruthy();
    expect(screen.getByText("New session")).toBeTruthy();
    expect(pages).toHaveBeenNthCalledWith(2, 100, "older-cursor");
    expect(screen.queryByRole("button", { name: /load older sessions/i })).toBeNull();
  });

  it("does not append an older page after the repository changes", async () => {
    vi.mocked(listRepositories).mockResolvedValue([
      repository("repo-a", "Repo A"),
      repository("repo-b", "Repo B"),
    ]);
    const olderA = deferred<{ sessions: SessionSummary[]; next_cursor: string | null }>();
    vi.mocked(listRepositorySessionsPage).mockImplementation((id, _limit, cursor) => {
      if (id === "repo-a" && cursor === null) {
        return Promise.resolve({
          sessions: [summary("a-new", "A newest")],
          next_cursor: "a-older",
        });
      }
      if (id === "repo-a") return olderA.promise;
      return Promise.resolve({
        sessions: [summary("b-new", "B newest")],
        next_cursor: null,
      });
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /Repo A/i }));
    expect(await screen.findByText("A newest")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /load older sessions/i }));
    fireEvent.click(screen.getByRole("button", { name: /Repo B/i }));
    expect(await screen.findByText("B newest")).toBeTruthy();

    await act(async () => {
      olderA.resolve({
        sessions: [summary("a-old", "A older")],
        next_cursor: null,
      });
    });
    expect(screen.queryByText("A older")).toBeNull();
    expect(screen.getByText("B newest")).toBeTruthy();
  });

  it("preserves the loaded session depth during a background refresh", async () => {
    const first = Array.from({ length: 100 }, (_, index) =>
      summary(`new-${index}`, `New ${index}`),
    );
    const older = Array.from({ length: 20 }, (_, index) =>
      summary(`old-${index}`, `Old ${index}`),
    );
    const pages = vi.mocked(listSessionsPage);
    pages.mockClear();
    pages
      .mockResolvedValueOnce({ sessions: first, next_cursor: "older" })
      .mockResolvedValueOnce({ sessions: older, next_cursor: null })
      .mockResolvedValueOnce({ sessions: [...first, ...older], next_cursor: null });

    render(<App />);
    await screen.findByText("New 0");
    fireEvent.click(screen.getByRole("button", { name: /load older sessions/i }));
    await screen.findByText("Old 19");

    vi.useFakeTimers();
    try {
      act(() => {
        scan.handler?.({
          discovered: 1,
          ingested: 1,
          skipped: 0,
          failed: 0,
          enriched: 1,
          done: true,
        });
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(300);
      });
      expect(pages).toHaveBeenLastCalledWith(120, null);
      expect(screen.getByText("Old 19")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the latest session detail when responses finish out of order", async () => {
    const sessionA = summary("session-a", "A session");
    const sessionB = summary("session-b", "B session");
    vi.mocked(listSessionsPage).mockResolvedValue({
      sessions: [sessionA, sessionB],
      next_cursor: null,
    });
    const detailA = deferred<SessionDetail | null>();
    const detailB = deferred<SessionDetail | null>();
    vi.mocked(getSession).mockImplementation((id) =>
      id === sessionA.id ? detailA.promise : detailB.promise,
    );

    render(<App />);
    fireEvent.click(await screen.findByText("A session"));
    fireEvent.click(screen.getByText("B session"));
    expect(screen.getByRole("status").textContent).toContain("Loading session");

    await act(async () => detailB.resolve(detail(sessionB)));
    expect(screen.getByRole("heading", { name: "B session", level: 2 })).toBeTruthy();

    await act(async () => detailA.resolve(detail(sessionA)));
    expect(screen.queryByRole("heading", { name: "A session", level: 2 })).toBeNull();
    expect(screen.getByRole("heading", { name: "B session", level: 2 })).toBeTruthy();
  });

  it("ignores an error from a superseded session request", async () => {
    const sessionA = summary("session-a", "A session");
    const sessionB = summary("session-b", "B session");
    vi.mocked(listSessionsPage).mockResolvedValue({
      sessions: [sessionA, sessionB],
      next_cursor: null,
    });
    const detailA = deferred<SessionDetail | null>();
    const detailB = deferred<SessionDetail | null>();
    vi.mocked(getSession).mockImplementation((id) =>
      id === sessionA.id ? detailA.promise : detailB.promise,
    );

    render(<App />);
    fireEvent.click(await screen.findByText("A session"));
    fireEvent.click(screen.getByText("B session"));
    await act(async () => detailB.resolve(detail(sessionB)));
    await act(async () => detailA.reject(new Error("stale session failure")));

    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("heading", { name: "B session", level: 2 })).toBeTruthy();
  });

  it("resets active repo, folder, search, and session when forgetting everything", async () => {
    const session = summary("session-1", "Initial session");
    vi.mocked(listSessionsPage).mockResolvedValue({
      sessions: [session],
      next_cursor: null,
    });
    vi.mocked(listRepositorySessionsPage).mockResolvedValue({
      sessions: [session],
      next_cursor: null,
    });
    vi.mocked(listRepositories).mockResolvedValue([
      {
        id: "repo-1",
        display_name: "lore",
        session_count: 1,
        worktree_count: 1,
        identity_confidence: "confirmed",
        primary_path: "/Users/dev/Lore",
        is_missing: false,
      },
    ]);
    vi.mocked(listFolders).mockResolvedValue([
      {
        id: "folder-1",
        name: "Sprint 1",
        session_count: 1,
        position: 0,
      },
    ]);
    vi.mocked(getSession).mockResolvedValue(detail(session));
    vi.spyOn(window, "confirm").mockReturnValue(true);

    render(<App />);

    // Select repository
    fireEvent.click(await screen.findByText("lore"));
    // Open session
    fireEvent.click(await screen.findByText("Initial session"));
    expect(await screen.findByRole("heading", { name: "Initial session", level: 2 })).toBeTruthy();

    // Open settings and click Forget everything
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeTruthy();

    // Mock empty response after forget
    vi.mocked(listRepositories).mockResolvedValue([]);
    vi.mocked(listFolders).mockResolvedValue([]);
    vi.mocked(listSessionsPage).mockResolvedValue({
      sessions: [],
      next_cursor: null,
    });
    vi.mocked(forgetEverything).mockResolvedValueOnce({
      blobs_removed: 3,
      source_paths: [],
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Forget everything" }));
    });

    expect(forgetEverything).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
    expect(screen.getByRole("status").textContent).toContain("Archive cleared (3 blob(s) removed).");
    expect(screen.queryByRole("heading", { name: "Initial session", level: 2 })).toBeNull();
  });
});
