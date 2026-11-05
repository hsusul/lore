import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { NoteDto, ProposalDto } from "../ipc";

vi.mock("../ipc", () => ({
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

beforeEach(() => {
  vi.resetAllMocks();
  m.listProposals.mockResolvedValue([pnpm, tests]);
  m.listNotes.mockResolvedValue([push]);
  m.accountName.mockResolvedValue("Henry Su");
  m.acceptProposal.mockResolvedValue("repos/016fc9809d74377f/use-pnpm-not-npm.md");
  m.rejectProposal.mockResolvedValue(undefined);
  m.retireNote.mockResolvedValue("global/never-push-to-main.md");
});

describe("MapHome", () => {
  it("opens on the next lesson to review, with what was said, and maps every lesson and note", async () => {
    render(<MapHome onSessions={() => {}} onSettings={() => {}} />);
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
    render(<MapHome onSessions={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    m.listProposals.mockResolvedValue([tests]);
    fireEvent.click(within(card).getByRole("button", { name: "Accept" }));
    await waitFor(() => expect(m.acceptProposal).toHaveBeenCalledWith("P1", undefined));
    expect(await screen.findByRole("heading", { name: "Run the tests before saying done" })).toBeTruthy();
    expect(screen.getByRole("status").textContent).toBe("Accepted. Agents get it from their next session.");
  });

  it("accepts an edited rule", async () => {
    render(<MapHome onSessions={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(within(card).getByRole("button", { name: "Edit" }));
    fireEvent.change(within(card).getByRole("textbox", { name: "Rule" }), { target: { value: "Use pnpm everywhere" } });
    fireEvent.click(within(card).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(m.acceptProposal).toHaveBeenCalledWith("P1", "Use pnpm everywhere"));
  });

  it("asks again later: the lesson leaves the queue for now", async () => {
    render(<MapHome onSessions={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(within(card).getByRole("button", { name: "Ask again if it recurs" }));
    expect(screen.getByRole("heading", { name: "Run the tests before saying done" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "1 lesson waiting" })).toBeTruthy();
    expect(m.acceptProposal).not.toHaveBeenCalled();
  });

  it("rejects a lesson that is not a rule", async () => {
    render(<MapHome onSessions={() => {}} onSettings={() => {}} />);
    const card = await screen.findByRole("region", { name: "Opened item" });
    m.listProposals.mockResolvedValue([tests]);
    fireEvent.click(within(card).getByRole("button", { name: "Reject" }));
    await waitFor(() => expect(m.rejectProposal).toHaveBeenCalledWith("P1", "Rejected in the Lore app"));
    expect(await screen.findByRole("heading", { name: "Run the tests before saying done" })).toBeTruthy();
  });

  it("points at /lore-lessons when no lesson waits", async () => {
    m.listProposals.mockResolvedValue([]);
    render(<MapHome onSessions={() => {}} onSettings={() => {}} />);
    expect(await screen.findByText("You're caught up")).toBeTruthy();
    expect(screen.getByRole("heading", { name: /Run \/lore-lessons/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /lessons? waiting/ })).toBeNull();
  });

  it("lists notes, and retires one", async () => {
    render(<MapHome onSessions={() => {}} onSettings={() => {}} />);
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
    render(<MapHome onSessions={() => {}} onSettings={() => {}} />);
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

  it("goes to sessions and settings from the rail", async () => {
    const onSessions = vi.fn();
    const onSettings = vi.fn();
    render(<MapHome onSessions={onSessions} onSettings={onSettings} />);
    await screen.findByRole("region", { name: "Opened item" });
    fireEvent.click(screen.getByRole("button", { name: "Sessions" }));
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(onSessions).toHaveBeenCalledTimes(1);
    expect(onSettings).toHaveBeenCalledTimes(1);
  });

  it("says when the lessons cannot be read, and tries again", async () => {
    m.listProposals.mockRejectedValueOnce("archive is locked");
    render(<MapHome onSessions={() => {}} onSettings={() => {}} />);
    expect(await screen.findByText("archive is locked")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("region", { name: "Opened item" })).toBeTruthy();
  });
});
