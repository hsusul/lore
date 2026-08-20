import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import SessionView from "./SessionView";
import type { GitObservationDto, SessionDetail } from "../ipc";

const detail: SessionDetail = {
  summary: {
    id: "s1",
    agent_id: "codex",
    title: "Add billing webhook",
    started_at: 1_700_000_000_000,
    ended_at: 1_700_000_100_000,
    message_count: 2,
    tool_call_count: 0,
    primary_model: "gpt-x",
    parse_status: "partial",
  },
  parse_note: "1 parser note(s); first: unknown event_msg: brand_new_event",
  segments: [],
  messages: [
    {
      id: "m0",
      seq: 0,
      role: "user",
      event_kind: "message",
      is_sidechain: false,
      ts: 1_700_000_000_000,
      model: null,
      parts: [{ ordinal: 0, kind: "text", text: "please add it", content_json: null, searchable: true }],
    },
    {
      id: "m1",
      seq: 1,
      role: "assistant",
      event_kind: "message",
      is_sidechain: false,
      ts: null,
      model: "gpt-x",
      parts: [
        { ordinal: 0, kind: "thinking", text: "secret plan text", content_json: null, searchable: false },
        { ordinal: 1, kind: "opaque", text: null, content_json: null, searchable: false },
        { ordinal: 2, kind: "text", text: "done", content_json: null, searchable: true },
      ],
    },
  ],
  file_events: [
    {
      id: "fe0",
      path: "billing/webhook.ts",
      change_kind: "edit",
      old_path: null,
      lines_added: 3,
      lines_removed: 1,
      source: "agent_patch",
      has_patch: true,
    },
  ],
  next_message_cursor: null,
};

const git: GitObservationDto[] = [
  {
    segment_id: "seg0",
    source: "agent_recorded",
    event_ts: 1_700_000_000_000,
    observed_at: 1_700_000_000_000,
    temporal_confidence: "near_event",
    branch: "billing",
    commit_sha: "3ab9f1aa22334455",
    remote_url_norm: "github.com/x/proj",
    is_dirty: null,
    commit_exists: null,
  },
  {
    segment_id: "seg0",
    source: "lore_reverified",
    event_ts: null,
    observed_at: 1_700_000_200_000,
    temporal_confidence: "retrospective",
    branch: "billing",
    commit_sha: "3ab9f1aa22334455",
    remote_url_norm: null,
    is_dirty: null,
    commit_exists: false,
  },
];

describe("SessionView", () => {
  it("shows an empty state without a session", () => {
    render(<SessionView detail={null} git={[]} />);
    expect(screen.getByText(/select a session/i)).toBeTruthy();
  });

  it("renders the header with a partial-parse badge", () => {
    render(<SessionView detail={detail} git={git} />);
    expect(screen.getByRole("heading", { name: /add billing webhook/i })).toBeTruthy();
    expect(screen.getByText("partial")).toBeTruthy();
  });

  it("explains what a partial parse recovered and why it degraded", () => {
    render(<SessionView detail={detail} git={git} />);

    expect(
      screen.getByRole("heading", { name: "This session was only partially read" }),
    ).toBeTruthy();
    expect(screen.getByText(/unknown event_msg: brand_new_event/i)).toBeTruthy();
    expect(screen.getByText("2", { selector: ".parse-notice__metric-value" })).toBeTruthy();
    expect(screen.getByText("Messages recovered")).toBeTruthy();
    expect(screen.getByText("Git context recovered")).toBeTruthy();
    expect(screen.getByText(/original agent log is untouched/i)).toBeTruthy();
  });

  it("uses a safe fallback when a failed parse has no diagnostic", () => {
    render(
      <SessionView
        detail={{
          ...detail,
          summary: { ...detail.summary, parse_status: "failed" },
          parse_note: null,
        }}
        git={[]}
      />,
    );

    expect(
      screen.getByRole("heading", { name: "This session could not be fully read" }),
    ).toBeTruthy();
    expect(screen.getByText(/could not safely normalize this source format/i)).toBeTruthy();
  });

  it("labels git observations by provenance and flags a missing commit", () => {
    render(<SessionView detail={detail} git={git} />);
    expect(screen.getByText("recorded by agent")).toBeTruthy();
    expect(screen.getByText("reverified")).toBeTruthy();
    expect(screen.getByText(/commit missing/i)).toBeTruthy();
  });

  it("collapses thinking and never renders opaque content", () => {
    render(<SessionView detail={detail} git={git} />);
    expect(screen.getByText(/thinking \(not indexed\)/i)).toBeTruthy();
    expect(screen.getByText(/encrypted content — not shown/i)).toBeTruthy();
    // The redacted opaque part must not leak any stored value.
    expect(screen.queryByText(/secret plan text/)).not.toBeNull(); // thinking is viewable
    expect(screen.getByText("done")).toBeTruthy();
  });

  it("lets each message collapse and starts very long messages collapsed", () => {
    const longDetail: SessionDetail = {
      ...detail,
      messages: [
        {
          ...detail.messages[0],
          parts: [
            {
              ordinal: 0,
              kind: "text",
              text: "x".repeat(2_001),
              content_json: null,
              searchable: true,
            },
          ],
        },
      ],
    };
    render(<SessionView detail={longDetail} git={[]} />);
    const message = screen.getByLabelText("message 0");
    expect(message.hasAttribute("open")).toBe(false);
    expect(screen.getByText(/long message · collapsed/i)).toBeTruthy();
    fireEvent.click(message.querySelector("summary")!);
    expect(message.hasAttribute("open")).toBe(true);
  });

  it("progressively renders large timelines in bounded pages", () => {
    const messages = Array.from({ length: 205 }, (_, seq) => ({
      ...detail.messages[0],
      id: `message-${seq}`,
      seq,
    }));
    render(<SessionView detail={{ ...detail, messages }} git={[]} />);
    expect(document.querySelectorAll(".msg")).toHaveLength(200);
    fireEvent.click(screen.getByRole("button", { name: /show more messages \(200 of 205\)/i }));
    expect(document.querySelectorAll(".msg")).toHaveLength(205);
  });

  it("resets the bounded timeline immediately when a different session opens", () => {
    const messages = Array.from({ length: 205 }, (_, seq) => ({
      ...detail.messages[0],
      id: `message-${seq}`,
      seq,
    }));
    const { rerender } = render(
      <SessionView detail={{ ...detail, messages }} git={[]} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /show more messages/i }));
    expect(document.querySelectorAll(".msg")).toHaveLength(205);

    rerender(
      <SessionView
        detail={{
          ...detail,
          summary: { ...detail.summary, id: "s2" },
          messages: messages.map((message) => ({ ...message, id: `next-${message.id}` })),
        }}
        git={[]}
      />,
    );

    expect(document.querySelectorAll(".msg")).toHaveLength(200);
    expect(screen.getByRole("button", { name: /show more messages \(200 of 205\)/i })).toBeTruthy();
  });

  it("shows a flagged-secret badge and wires copy/save/forget", () => {
    const onExport = vi.fn();
    const onSaveFile = vi.fn();
    const onForget = vi.fn();
    render(
      <SessionView
        detail={detail}
        git={git}
        secretCount={2}
        onExport={onExport}
        onSaveFile={onSaveFile}
        onForget={onForget}
      />,
    );
    expect(screen.getByText(/2 flagged/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /copy markdown/i }));
    expect(onExport).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /save file/i }));
    expect(onSaveFile).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /forget/i }));
    expect(onForget).toHaveBeenCalled();
  });

  it("lists file events with a diffstat and loads the patch inline on demand", async () => {
    const loadPatch = vi.fn().mockResolvedValue("@@ -1 +1 @@\n-old\n+new\n");
    render(<SessionView detail={detail} git={git} loadPatch={loadPatch} />);
    expect(screen.getByText("billing/webhook.ts")).toBeTruthy();
    expect(screen.getByText(/\+3/)).toBeTruthy();

    // The recorded patch is fetched only when the user asks to view it.
    fireEvent.click(screen.getByRole("button", { name: /view patch/i }));
    expect(loadPatch).toHaveBeenCalledWith("fe0");
    await waitFor(() => expect(screen.getByText(/\+new/)).toBeTruthy());
  });

  it("bounds oversized diffs to 1,000 lines with a truncation note", async () => {
    const largeDiff = Array.from({ length: 1_200 }, (_, i) => `+line ${i}`).join("\n");
    const loadPatch = vi.fn().mockResolvedValue(largeDiff);
    render(<SessionView detail={detail} git={git} loadPatch={loadPatch} />);

    fireEvent.click(screen.getByRole("button", { name: /view patch/i }));
    await waitFor(() =>
      expect(screen.getByText(/… truncated \(200 more lines\)/)).toBeTruthy(),
    );
  });

  it("displays segment count for multi-segment sessions and handles untitled fallback", () => {
    render(
      <SessionView
        detail={{
          ...detail,
          summary: { ...detail.summary, title: null },
          segments: [
            {
              id: "seg0",
              seq_start: 0,
              seq_end: 0,
              cwd: "/repo/a",
              model: "gpt-4",
              provider: null,
              repository_id: null,
              resolution_confidence: "unresolved",
            },
            {
              id: "seg1",
              seq_start: 1,
              seq_end: 1,
              cwd: "/repo/b",
              model: "gpt-4",
              provider: null,
              repository_id: null,
              resolution_confidence: "unresolved",
            },
          ],
        }}
        git={[]}
      />,
    );

    expect(screen.getByRole("heading", { name: "(untitled session)" })).toBeTruthy();
    expect(screen.getByText("2 segments")).toBeTruthy();
  });
});
