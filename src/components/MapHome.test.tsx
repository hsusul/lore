import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { NoteDto, ProposalDto, SessionSummary } from "../ipc";

vi.mock("../ipc", () => ({
  HIGHLIGHT_START: "\u{e000}",
  HIGHLIGHT_END: "\u{e001}",
  listSessionsPage: vi.fn(),
  searchPage: vi.fn(),
  getSession: vi.fn(),
  listSessionMessagesPage: vi.fn(),
  listProposals: vi.fn(),
  findLessons: vi.fn(),
  lessonAgents: vi.fn(),
  listNotes: vi.fn(),
  accountName: vi.fn(),
  acceptProposal: vi.fn(),
  rejectProposal: vi.fn(),
  retireNote: vi.fn(),
}));

// The canvas draws the highlight; this stand-in shows which dot it is given,
// and a click on it is a click off every dot.
vi.mock("./MapCanvas", () => ({
  MapCanvas: ({
    label,
    selectedId,
    onDeselect,
    homeKey,
  }: {
    label: string;
    selectedId: string | null;
    onDeselect: () => void;
    homeKey?: number;
  }) => (
    <div
      role="img"
      aria-label={label}
      data-selected={selectedId ?? ""}
      data-home={homeKey ?? 0}
      onClick={onDeselect}
    />
  ),
}));

import * as ipc from "../ipc";
import { MapHome } from "./MapHome";

const m = vi.mocked(ipc);

const pnpm: ProposalDto = {
  id: "P1",
  title: "Use pnpm, not npm",
  body: 'The agent ran npm install, and the person wanted pnpm.\n\nReceipts:\n- "no, use pnpm not npm" (claude-code, session s1, 2026-10-12)\n- "pnpm, again" (codex, session s2, 2026-10-20)\n',
  scope: "repo:016fc9809d74377f",
  note_type: "convention",
  agent_id: "claude-code",
  session_id: "s1",
  repo_name: "lore",
  proposed_ms: 1_785_751_203_000,
  enforce: [],
  updates: null,
  target_enforce: [],
  repeats: null,
};
const tests: ProposalDto = {
  ...pnpm,
  id: "P2",
  title: "Run the tests before saying done",
  body: "The agent said it was done before testing.\n",
};
const push: NoteDto = {
  id: "N1",
  path: "global/never-push-to-main.md",
  title: "Never push to main",
  scope: "global",
  note_type: "convention",
  excerpt: "Open a PR.",
  repo_name: null,
  enforce: [],
  dropped_rules: [],
};

const summary = (id: string, title: string): SessionSummary => ({
  id,
  agent_id: "claude-code",
  title,
  started_at: 1_785_751_203_000,
  ended_at: 1_785_751_203_000 + 48 * 60_000,
  message_count: 5,
  tool_call_count: 1,
  primary_model: null,
  parse_status: "ok",
  total_input_tokens: null,
  total_output_tokens: null,
  total_cache_tokens: null,
  est_cost_usd: null,
});

const agent = (
  id: string,
  label: string,
  signed_in: boolean,
  installed = true,
) => ({
  id,
  label,
  installed,
  signed_in,
  sign_in: id === "codex" ? "codex login" : "claude auth login",
});

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  m.lessonAgents.mockResolvedValue([
    agent("claude-code", "Claude Code", true),
    agent("codex", "Codex", true),
  ]);
  m.listSessionsPage.mockResolvedValue({
    sessions: [
      summary("s1", "Add analytics to onboarding"),
      summary("s2", "Fix the retry backoff"),
    ],
    next_cursor: null,
  });
  m.searchPage.mockResolvedValue({
    hits: [
      {
        session_id: "s2",
        source_kind: "message_part",
        source_id: "p1",
        field: "text",
        snippet: "make the \u{e000}retry\u{e001} deterministic",
        rank: -1,
        title: "Fix the retry backoff",
        agent_id: "codex",
        started_at: 1_785_751_203_000,
      },
    ],
    next_cursor: null,
  });
  m.getSession.mockResolvedValue({
    summary: summary("s1", "Add analytics to onboarding"),
    parse_note: null,
    segments: [],
    file_events: [],
    next_message_cursor: null,
    messages: [
      {
        id: "m1",
        seq: 0,
        role: "user",
        event_kind: "message",
        is_sidechain: false,
        ts: null,
        model: null,
        parts: [
          {
            ordinal: 0,
            kind: "text",
            text: "add analytics events",
            content_json: null,
            searchable: true,
          },
        ],
      },
      {
        id: "m2",
        seq: 1,
        role: "assistant",
        event_kind: "message",
        is_sidechain: false,
        ts: null,
        model: null,
        parts: [
          {
            ordinal: 0,
            kind: "tool_use",
            text: null,
            content_json: '{"name":"Edit"}',
            searchable: false,
          },
        ],
      },
      {
        id: "m3",
        seq: 2,
        role: "assistant",
        event_kind: "message",
        is_sidechain: false,
        ts: null,
        model: null,
        parts: [
          {
            ordinal: 0,
            kind: "text",
            text: "Added the events.",
            content_json: null,
            searchable: true,
          },
        ],
      },
    ],
  });
  m.listProposals.mockResolvedValue([pnpm, tests]);
  m.listNotes.mockResolvedValue([push]);
  m.accountName.mockResolvedValue("Henry Su");
  m.acceptProposal.mockResolvedValue(
    "repos/016fc9809d74377f/use-pnpm-not-npm.md",
  );
  m.rejectProposal.mockResolvedValue(undefined);
  m.retireNote.mockResolvedValue("global/never-push-to-main.md");
});

describe("MapHome", () => {
  it("opens on the next lesson to review, with what was said, and maps every lesson and note", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    expect(
      within(card).getByRole("heading", { name: "Use pnpm, not npm" }),
    ).toBeTruthy();
    expect(within(card).getByText("lore")).toBeTruthy();
    expect(
      within(card).getByText(
        "The agent ran npm install, and the person wanted pnpm.",
      ),
    ).toBeTruthy();
    expect(within(card).getByText("“pnpm, again”")).toBeTruthy();
    expect(
      within(card).getByText("codex, session s2, 2026-10-20"),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "2 lessons waiting" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("img", {
        name: "Map of 3 lessons and notes in 2 areas",
      }),
    ).toBeTruthy();
  });

  it("accepts the lesson and moves on to the next", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    m.listProposals.mockResolvedValue([tests]);
    fireEvent.click(within(card).getByRole("button", { name: "Accept" }));
    await waitFor(() =>
      expect(m.acceptProposal).toHaveBeenCalledWith("P1", undefined),
    );
    expect(
      await screen.findByRole("heading", {
        name: "Run the tests before saying done",
      }),
    ).toBeTruthy();
    expect(screen.getByRole("status").textContent).toBe(
      "Accepted. Agents get it from their next session.",
    );
  });

  it("accepts an edited rule", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(within(card).getByRole("button", { name: "Edit" }));
    fireEvent.change(within(card).getByRole("textbox", { name: "Rule" }), {
      target: { value: "Use pnpm everywhere" },
    });
    fireEvent.click(within(card).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(m.acceptProposal).toHaveBeenCalledWith(
        "P1",
        "Use pnpm everywhere",
      ),
    );
  });

  it("asks again later: the lesson leaves the queue for now", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(
      within(card).getByRole("button", { name: "Ask again if it recurs" }),
    );
    expect(
      screen.getByRole("heading", { name: "Run the tests before saying done" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "1 lesson waiting" }),
    ).toBeTruthy();
    expect(m.acceptProposal).not.toHaveBeenCalled();
  });

  it("rejects a lesson that is not a rule", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    m.listProposals.mockResolvedValue([tests]);
    fireEvent.click(within(card).getByRole("button", { name: "Reject" }));
    await waitFor(() =>
      expect(m.rejectProposal).toHaveBeenCalledWith(
        "P1",
        "Rejected in the Lore app",
      ),
    );
    expect(
      await screen.findByRole("heading", {
        name: "Run the tests before saying done",
      }),
    ).toBeTruthy();
  });

  it("marks a lesson that repeats another, and rejects it as a duplicate", async () => {
    m.listProposals.mockResolvedValue([
      {
        ...pnpm,
        repeats: { id: "N1", title: "Never push to main", kind: "note" },
      },
      {
        ...tests,
        repeats: { id: "P0", title: "Run tests before done", kind: "proposal" },
      },
    ]);
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    expect(
      within(card).getByText("Already a note: Never push to main"),
    ).toBeTruthy();
    m.listProposals.mockResolvedValue([
      {
        ...tests,
        repeats: { id: "P0", title: "Run tests before done", kind: "proposal" },
      },
    ]);
    fireEvent.click(
      within(card).getByRole("button", { name: "Reject as duplicate" }),
    );
    await waitFor(() =>
      expect(m.rejectProposal).toHaveBeenCalledWith("P1", "duplicate of N1"),
    );
    expect(
      await screen.findByText(
        "Same as a lesson you haven't reviewed: Run tests before done",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("status").textContent).toBe(
      "Rejected as a duplicate.",
    );
  });

  it("shows no duplicate line or action for a lesson that repeats nothing", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    expect(
      within(card).queryByRole("button", { name: "Reject as duplicate" }),
    ).toBeNull();
  });

  it("finds lessons with the chosen agent when none wait, then shows them", async () => {
    m.listProposals.mockResolvedValue([]);
    let finish: (report: string) => void = () => {};
    m.findLessons.mockReturnValue(new Promise((resolve) => (finish = resolve)));
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    expect(await screen.findByText("You're caught up")).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: /lessons? waiting/ }),
    ).toBeNull();

    const choice = await screen.findByRole("radiogroup", {
      name: "Agent that writes lessons",
    });
    fireEvent.click(within(choice).getByRole("radio", { name: "Codex" }));
    expect(localStorage.getItem("lore.lessons-agent")).toBe("codex");
    fireEvent.click(
      screen.getByRole("button", { name: "Find lessons with Codex" }),
    );
    expect(m.findLessons).toHaveBeenCalledWith("codex");
    expect(
      await screen.findByRole("heading", {
        name: /Finding lessons Codex is reading/,
      }),
    ).toBeTruthy();
    m.listProposals.mockResolvedValue([pnpm]);
    finish("Filed: 1 rule.");
    expect(
      await screen.findByRole("heading", { name: "Use pnpm, not npm" }),
    ).toBeTruthy();
    expect(screen.getByText("Codex filed 1 lesson.")).toBeTruthy();
  });

  it("finds more lessons from a lesson, while others still wait", async () => {
    m.findLessons.mockResolvedValue("done");
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(
      within(card).getByRole("button", { name: "Find more lessons" }),
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Find lessons with Claude Code",
      }),
    );
    await waitFor(() =>
      expect(m.findLessons).toHaveBeenCalledWith("claude-code"),
    );
    expect(
      await screen.findByText("Claude Code found no new lessons."),
    ).toBeTruthy();
    expect(
      screen.getByRole("heading", { name: "Use pnpm, not npm" }),
    ).toBeTruthy();
  });

  it("offers only the agent that is signed in", async () => {
    m.listProposals.mockResolvedValue([]);
    m.lessonAgents.mockResolvedValue([
      agent("claude-code", "Claude Code", true),
      agent("codex", "Codex", false),
    ]);
    m.findLessons.mockResolvedValue("done");
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Find lessons with Claude Code",
      }),
    );
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(m.findLessons).toHaveBeenCalledWith("claude-code");
  });

  it("asks the person to sign in when no agent can write lessons", async () => {
    m.listProposals.mockResolvedValue([]);
    m.lessonAgents.mockResolvedValue([
      agent("claude-code", "Claude Code", false),
      agent("codex", "Codex", false, false),
    ]);
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    expect(
      await screen.findByRole("heading", {
        name: /Sign in to Claude Code or Codex/,
      }),
    ).toBeTruthy();
    expect(screen.getByText("claude auth login")).toBeTruthy();
    expect(screen.getByText("Not installed")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Find lessons/ })).toBeNull();

    m.lessonAgents.mockResolvedValue([
      agent("claude-code", "Claude Code", true),
      agent("codex", "Codex", false, false),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    expect(
      await screen.findByRole("button", {
        name: "Find lessons with Claude Code",
      }),
    ).toBeTruthy();
  });

  it("says why finding lessons failed", async () => {
    m.listProposals.mockResolvedValue([]);
    m.findLessons.mockRejectedValue(
      "Claude Code did not finish its report: Not logged in",
    );
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Find lessons with Claude Code",
      }),
    );
    expect(
      await screen.findByText(
        "Claude Code did not finish its report: Not logged in",
      ),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Find lessons with Claude Code" }),
    ).toBeTruthy();
  });

  it("lets go of the highlighted dot when the card closes or the map is clicked off a dot", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    await screen.findByRole("region", { name: "Opened item" });
    const map = screen.getByRole("img", { name: /^Map of/ });
    expect(map.dataset.selected).toBe("lesson:P1");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(map.dataset.selected).toBe("");

    fireEvent.click(screen.getByRole("button", { name: "Notes" }));
    fireEvent.click(
      within(screen.getByRole("region", { name: "Notes" })).getByRole(
        "button",
        { name: /Never push to main/ },
      ),
    );
    expect(map.dataset.selected).toBe("note:N1");

    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(map.dataset.selected).toBe("");

    fireEvent.click(screen.getByRole("button", { name: "Notes" }));
    fireEvent.click(
      within(screen.getByRole("region", { name: "Notes" })).getByRole(
        "button",
        { name: /Never push to main/ },
      ),
    );
    fireEvent.click(map);
    expect(map.dataset.selected).toBe("");
    expect(
      screen.queryByRole("heading", { name: "Never push to main" }),
    ).toBeNull();
  });

  it("lists notes, and retires one", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(screen.getByRole("button", { name: "Notes" }));
    const list = screen.getByRole("region", { name: "Notes" });
    fireEvent.click(
      within(list).getByRole("button", { name: /Never push to main/ }),
    );
    const card = screen.getByRole("region", { name: "Opened item" });
    expect(within(card).getByText("Henry")).toBeTruthy();
    fireEvent.click(within(card).getByRole("button", { name: "Retire" }));
    await waitFor(() =>
      expect(m.retireNote).toHaveBeenCalledWith(
        "N1",
        "Retired in the Lore app",
      ),
    );
  });

  it("shows the rules a lesson adds before it is accepted, and none for a lesson without", async () => {
    const rule = { tool: "bash", pattern: "\\bnpm install\\b", unless: "--prefix" };
    m.listProposals.mockResolvedValue([{ ...pnpm, enforce: [rule] }, tests]);
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    const blocks = within(card).getByRole("region", { name: "Blocks" });
    expect(within(blocks).getByText("\\bnpm install\\b").tagName).toBe("CODE");
    expect(within(blocks).getByText("--prefix")).toBeTruthy();
    expect(within(blocks).getByText(/Once accepted/)).toBeTruthy();
    expect(within(blocks).queryByText("new")).toBeNull();

    fireEvent.click(within(card).getByRole("button", { name: "Ask again if it recurs" }));
    const next = await screen.findByRole("heading", { name: "Run the tests before saying done" });
    expect(next).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Blocks" })).toBeNull();
  });

  it("marks which rules an update adds to its note", async () => {
    const kept = { tool: "bash", pattern: "git push.*main", unless: null };
    const added = { tool: "path", pattern: "^\\.github/", unless: null };
    m.listProposals.mockResolvedValue([
      { ...pnpm, enforce: [kept, added], updates: "N1", target_enforce: [kept] },
    ]);
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    const rows = within(within(card).getByRole("region", { name: "Blocks" })).getAllByRole("listitem");
    expect(rows.map((r) => r.textContent)).toEqual(["bashgit push.*main", "path^\\.github/new"]);
  });

  it("shows a note's rules, and a rule that does not parse as not enforced", async () => {
    m.listNotes.mockResolvedValue([
      {
        ...push,
        enforce: [{ tool: "bash", pattern: "git push.*main", unless: "--dry-run" }],
        dropped_rules: [{ written: "tool: shell\npattern: 'x'", error: "`enforce[1].tool`: expected bash, write, or path, got `shell`" }],
      },
    ]);
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(screen.getByRole("button", { name: "Notes" }));
    fireEvent.click(
      within(screen.getByRole("region", { name: "Notes" })).getByRole("button", { name: /Never push to main/ }),
    );
    const blocks = within(screen.getByRole("region", { name: "Opened item" })).getByRole("region", { name: "Blocks" });
    expect(within(blocks).getByText("git push.*main")).toBeTruthy();
    expect(within(blocks).getByText("Agents' tool calls that match are denied.")).toBeTruthy();
    expect(within(blocks).getByText("not enforced")).toBeTruthy();
    expect(within(blocks).getByText(/got `shell`/)).toBeTruthy();
  });

  it("closes the card to look around; the logo or the pill brings it back", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("region", { name: "Opened item" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "2 lessons waiting" }));
    expect(screen.getByRole("region", { name: "Opened item" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Today" }));
    expect(screen.queryByRole("region", { name: "Opened item" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Today" }));
    expect(screen.getByRole("region", { name: "Opened item" })).toBeTruthy();
  });

  it("zooms the map back out to the whole map when the logo is pressed", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const map = await screen.findByRole("img", { name: /^Map of/ });
    const before = Number(map.getAttribute("data-home"));
    fireEvent.click(screen.getByRole("button", { name: "Today" }));
    expect(Number(map.getAttribute("data-home"))).toBe(before + 1);
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(Number(map.getAttribute("data-home"))).toBe(before + 1);
  });

  it("lists sessions in the card and opens one there, with a way back", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(screen.getByRole("button", { name: "Sessions" }));
    const card = screen.getByRole("region", { name: "Sessions" });
    fireEvent.click(
      await within(card).findByRole("button", {
        name: /Add analytics to onboarding/,
      }),
    );

    const reader = await within(card).findByRole("article", {
      name: "Add analytics to onboarding",
    });
    expect(within(reader).getByText("add analytics events")).toBeTruthy();
    expect(within(reader).getByText("Ran Edit")).toBeTruthy();
    expect(within(reader).getByText("Added the events.")).toBeTruthy();
    expect(within(reader).getByText(/Claude Code, .*, 48 min/)).toBeTruthy();

    fireEvent.click(within(card).getByRole("button", { name: "‹ Sessions" }));
    expect(
      await within(card).findByRole("button", {
        name: /Fix the retry backoff/,
      }),
    ).toBeTruthy();
  });

  it("searches sessions from the rail and shows the matching snippet", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    const field = screen.getByRole("searchbox", { name: "Search sessions" });
    expect(document.activeElement).toBe(field);
    fireEvent.change(field, { target: { value: "retry" } });
    await waitFor(() =>
      expect(m.searchPage).toHaveBeenCalledWith("retry", 50, null),
    );
    const row = await screen.findByRole("button", {
      name: /Fix the retry backoff/,
    });
    expect(within(row).getByText("retry").tagName).toBe("MARK");
  });

  it("opens the full archive and settings from the card and the rail", async () => {
    const onArchive = vi.fn();
    const onSettings = vi.fn();
    render(<MapHome onArchive={onArchive} onSettings={onSettings} />);
    await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(screen.getByRole("button", { name: "Sessions" }));
    fireEvent.click(screen.getByRole("button", { name: /full archive/ }));
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(onArchive).toHaveBeenCalledTimes(1);
    expect(onSettings).toHaveBeenCalledTimes(1);
  });

  it("says when the lessons cannot be read, and tries again", async () => {
    m.listProposals.mockRejectedValueOnce("archive is locked");
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    expect(await screen.findByText("archive is locked")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(
      await screen.findByRole("region", { name: "Opened item" }),
    ).toBeTruthy();
  });
});
