import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { EnforceRuleDto, NoteDto, ProposalDto } from "../ipc";
import { createMockHandlers } from "../mock/ipc-mock";
import { Blocks } from "./Blocks";

const frob: EnforceRuleDto = { tool: "bash", pattern: "frobnicate --all", unless: "--dry-run" };
const widgets: EnforceRuleDto = { tool: "path", pattern: "^widgets/", unless: null };
const old: EnforceRuleDto = { tool: "write", pattern: "frob\\(", unless: null };

const items = (section: HTMLElement) =>
  within(section)
    .getAllByRole("listitem")
    .map((li) => li.textContent);

describe("Blocks", () => {
  it("shows each rule's tool, pattern in monospace, and unless, and what accepting does", () => {
    render(<Blocks rules={[frob, widgets]} pending />);
    const section = screen.getByRole("region", { name: "Blocks" });
    expect(items(section)).toEqual(["bashfrobnicate --allunless--dry-run", "path^widgets/"]);
    expect(within(section).getByText("frobnicate --all").tagName).toBe("CODE");
    expect(within(section).getByText("Once accepted, agents' tool calls that match are denied.")).toBeTruthy();
  });

  it("marks an update's new rules and the target's rules it drops", () => {
    render(<Blocks rules={[frob, widgets]} before={[frob, old]} pending />);
    const section = screen.getByRole("region", { name: "Blocks" });
    expect(items(section)).toEqual([
      "bashfrobnicate --allunless--dry-run",
      "path^widgets/new",
      "writefrob\\(removed",
    ]);
  });

  it("marks a rule that did not parse as not enforced", () => {
    render(
      <Blocks
        rules={[]}
        dropped={[{ written: "tool: bash\npattern: '('", error: "`enforce[0].pattern`: `(` is not a valid regex" }]}
      />,
    );
    const section = screen.getByRole("region", { name: "Blocks" });
    expect(within(section).getByText("not enforced")).toBeTruthy();
    expect(within(section).getByText("`enforce[0].pattern`: `(` is not a valid regex")).toBeTruthy();
    expect(within(section).queryByText(/are denied/)).toBeNull();
  });

  it("shows nothing without rules", () => {
    const { container } = render(<Blocks rules={[]} before={[]} />);
    expect(container.innerHTML).toBe("");
  });

  it("shows the mock IPC's rules: an update's new rule and a note's dropped one", async () => {
    const handlers = createMockHandlers(() => {}, { now: Date.UTC(2026, 0, 1), scanStepMs: 0 });
    const proposals = (await handlers.list_proposals({})) as ProposalDto[];
    const notes = (await handlers.list_notes({})) as NoteDto[];
    const update = proposals.find((p) => p.updates !== null)!;
    expect(notes.find((n) => n.id === update.updates)?.enforce).toEqual(update.target_enforce);
    render(<Blocks rules={update.enforce} before={update.target_enforce} pending />);
    expect(screen.getAllByText("new")).toHaveLength(1);
    const brew = notes.find((n) => n.dropped_rules.length > 0)!;
    render(<Blocks rules={brew.enforce} dropped={brew.dropped_rules} />);
    expect(screen.getByText("not enforced")).toBeTruthy();
  });
});
