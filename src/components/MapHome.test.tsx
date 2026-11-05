import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  listNotes: vi.fn(),
  accountName: vi.fn(),
  acceptProposal: vi.fn(),
  rejectProposal: vi.fn(),
  retireNote: vi.fn(),
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
};
const tests: ProposalDto = { ...pnpm, id: "P2", title: "Run the tests before saying done", body: "The agent said it was done before testing.\n" };
const push: NoteDto = {
  id: "N1",
  path: "global/never-push-to-main.md",
  title: "Never push to main",
  scope: "global",
  note_type: "convention",
  excerpt: "Open a PR.",
  repo_name: null,
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

beforeEach(() => {
  vi.resetAllMocks();
  m.listSessionsPage.mockResolvedValue({ sessions: [summary("s1", "Add analytics to onboarding"), summary("s2", "Fix the retry backoff")], next_cursor: null });
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
      { id: "m1", seq: 0, role: "user", event_kind: "message", is_sidechain: false, ts: null, model: null, parts: [{ ordinal: 0, kind: "text", text: "add analytics events", content_json: null, searchable: true }] },
      { id: "m2", seq: 1, role: "assistant", event_kind: "message", is_sidechain: false, ts: null, model: null, parts: [{ ordinal: 0, kind: "tool_use", text: null, content_json: '{"name":"Edit"}', searchable: false }] },
      { id: "m3", seq: 2, role: "assistant", event_kind: "message", is_sidechain: false, ts: null, model: null, parts: [{ ordinal: 0, kind: "text", text: "Added the events.", content_json: null, searchable: true }] },
    ],
  });
  m.listProposals.mockResolvedValue([pnpm, tests]);
  m.listNotes.mockResolvedValue([push]);
  m.accountName.mockResolvedValue("Henry Su");
  m.acceptProposal.mockResolvedValue("repos/016fc9809d74377f/use-pnpm-not-npm.md");
  m.rejectProposal.mockResolvedValue(undefined);
  m.retireNote.mockResolvedValue("global/never-push-to-main.md");
});

describe("MapHome", () => {
  it("opens on the next lesson to review, with what was said, and maps every lesson and note", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    expect(within(card).getByRole("heading", { name: "Use pnpm, not npm" })).toBeTruthy();
    expect(within(card).getByText("lore")).toBeTruthy();
    expect(within(card).getByText("The agent ran npm install, and the person wanted pnpm.")).toBeTruthy();
    expect(within(card).getByText("“pnpm, again”")).toBeTruthy();
    expect(within(card).getByText("codex, session s2, 2026-10-20")).toBeTruthy();
    expect(screen.getByRole("button", { name: "2 lessons waiting" })).toBeTruthy();
    expect(screen.getByRole("img", { name: "Map of 3 lessons and notes in 2 areas" })).toBeTruthy();
  });

  it("accepts the lesson and moves on to the next", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    m.listProposals.mockResolvedValue([tests]);
    fireEvent.click(within(card).getByRole("button", { name: "Accept" }));
    await waitFor(() => expect(m.acceptProposal).toHaveBeenCalledWith("P1", undefined));
    expect(await screen.findByRole("heading", { name: "Run the tests before saying done" })).toBeTruthy();
    expect(screen.getByRole("status").textContent).toBe("Accepted. Agents get it from their next session.");
  });

  it("accepts an edited rule", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(within(card).getByRole("button", { name: "Edit" }));
    fireEvent.change(within(card).getByRole("textbox", { name: "Rule" }), { target: { value: "Use pnpm everywhere" } });
    fireEvent.click(within(card).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(m.acceptProposal).toHaveBeenCalledWith("P1", "Use pnpm everywhere"));
  });

  it("asks again later: the lesson leaves the queue for now", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(within(card).getByRole("button", { name: "Ask again if it recurs" }));
    expect(screen.getByRole("heading", { name: "Run the tests before saying done" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "1 lesson waiting" })).toBeTruthy();
    expect(m.acceptProposal).not.toHaveBeenCalled();
  });

  it("rejects a lesson that is not a rule", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    m.listProposals.mockResolvedValue([tests]);
    fireEvent.click(within(card).getByRole("button", { name: "Reject" }));
    await waitFor(() => expect(m.rejectProposal).toHaveBeenCalledWith("P1", "Rejected in the Lore app"));
    expect(await screen.findByRole("heading", { name: "Run the tests before saying done" })).toBeTruthy();
  });

  it("points at /lore-lessons when no lesson waits", async () => {
    m.listProposals.mockResolvedValue([]);
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    expect(await screen.findByText("You're caught up")).toBeTruthy();
    expect(screen.getByRole("heading", { name: /Run \/lore-lessons/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /lessons? waiting/ })).toBeNull();
  });

  it("lists notes, and retires one", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(screen.getByRole("button", { name: "Notes" }));
    const list = screen.getByRole("region", { name: "Notes" });
    fireEvent.click(within(list).getByRole("button", { name: /Never push to main/ }));
    const card = screen.getByRole("region", { name: "Opened item" });
    expect(within(card).getByText("Henry")).toBeTruthy();
    fireEvent.click(within(card).getByRole("button", { name: "Retire" }));
    await waitFor(() => expect(m.retireNote).toHaveBeenCalledWith("N1", "Retired in the Lore app"));
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

  it("lists sessions in the card and opens one there, with a way back", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(screen.getByRole("button", { name: "Sessions" }));
    const card = screen.getByRole("region", { name: "Sessions" });
    fireEvent.click(await within(card).findByRole("button", { name: /Add analytics to onboarding/ }));

    const reader = await within(card).findByRole("article", { name: "Add analytics to onboarding" });
    expect(within(reader).getByText("add analytics events")).toBeTruthy();
    expect(within(reader).getByText("Ran Edit")).toBeTruthy();
    expect(within(reader).getByText("Added the events.")).toBeTruthy();
    expect(within(reader).getByText(/Claude Code, .*, 48 min/)).toBeTruthy();

    fireEvent.click(within(card).getByRole("button", { name: "‹ Sessions" }));
    expect(await within(card).findByRole("button", { name: /Fix the retry backoff/ })).toBeTruthy();
  });

  it("searches sessions from the rail and shows the matching snippet", async () => {
    render(<MapHome onArchive={() => {}} onSettings={() => {}} />);
    await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    const field = screen.getByRole("searchbox", { name: "Search sessions" });
    expect(document.activeElement).toBe(field);
    fireEvent.change(field, { target: { value: "retry" } });
    await waitFor(() => expect(m.searchPage).toHaveBeenCalledWith("retry", 50, null));
    const row = await screen.findByRole("button", { name: /Fix the retry backoff/ });
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
    expect(await screen.findByRole("region", { name: "Opened item" })).toBeTruthy();
  });
});
