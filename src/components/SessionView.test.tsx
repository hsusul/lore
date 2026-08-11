import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

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
      path: "billing/webhook.ts",
      change_kind: "edit",
      old_path: null,
      lines_added: 3,
      lines_removed: 1,
      source: "agent_patch",
      has_patch: true,
    },
  ],
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

  it("lists file events with a recorded-patch badge and diffstat", () => {
    render(<SessionView detail={detail} git={git} />);
    expect(screen.getByText("billing/webhook.ts")).toBeTruthy();
    expect(screen.getByText("patch stored")).toBeTruthy();
    expect(screen.getByText(/\+3/)).toBeTruthy();
  });
});
