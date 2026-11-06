import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { SkillUsageDto } from "../ipc";

vi.mock("../ipc", () => ({
  lessonAgents: vi.fn(),
  draftSkillEdit: vi.fn(),
  applySkillEdit: vi.fn(),
}));

import * as ipc from "../ipc";
import { SkillCard } from "./SkillCard";

const m = vi.mocked(ipc);

const review: SkillUsageDto = {
  skill: "code-review",
  uses: 22,
  sessions: 15,
  last_ms: 1_785_751_203_000,
  reprompts: [
    {
      session_id: "s1",
      agent_id: "claude-code",
      repo: "lore",
      at_ms: 1_785_751_203_000,
      text: "no, you missed the tests",
      preceding: "Looks fine.",
    },
  ],
  editable_path: "/Users/me/.claude/skills/code-review/SKILL.md",
};
const draft = {
  path: "/Users/me/.claude/skills/code-review/SKILL.md",
  summary: "Read the tests before approving a change.",
  before: "---\nname: code-review\n---\nReview the diff.\n",
  after: "---\nname: code-review\n---\nReview the diff and its tests.\n",
};

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  m.lessonAgents.mockResolvedValue([
    {
      id: "claude-code",
      label: "Claude Code",
      installed: true,
      signed_in: true,
      sign_in: "claude auth login",
    },
  ]);
  m.draftSkillEdit.mockResolvedValue(draft);
  m.applySkillEdit.mockResolvedValue("/backups/code-review-1.SKILL.md");
});

describe("SkillCard", () => {
  it("shows use, what was said after it ran, and the doctor's finding", async () => {
    render(
      <SkillCard
        skill={review}
        findings={[
          {
            kind: "name-mismatch",
            skill: "code-review",
            description: "SKILL.md names it review.",
            fix: "Rename it.",
            fix_kind: "edit",
          },
        ]}
        onApplied={() => {}}
      />,
    );
    expect(screen.getByRole("heading", { name: "code-review" })).toBeTruthy();
    expect(screen.getByText(/22 uses in 15 sessions, last/)).toBeTruthy();
    expect(screen.getByText("“no, you missed the tests”")).toBeTruthy();
    expect(screen.getByText("SKILL.md names it review.")).toBeTruthy();
    expect(screen.getByText("Rename it.")).toBeTruthy();
  });

  it("drafts an edit with the agent, shows the change, and applies it", async () => {
    const onApplied = vi.fn();
    render(<SkillCard skill={review} findings={[]} onApplied={onApplied} />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Propose an edit with Claude Code",
      }),
    );
    expect(m.draftSkillEdit).toHaveBeenCalledWith("claude-code", "code-review");
    expect(
      await screen.findByText("Read the tests before approving a change."),
    ).toBeTruthy();
    const changes = screen.getByLabelText("Changes");
    expect(within(changes).getByText("- Review the diff.").className).toBe(
      "del",
    );
    expect(
      within(changes).getByText("+ Review the diff and its tests.").className,
    ).toBe("add");

    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(m.applySkillEdit).toHaveBeenCalledWith(
        "code-review",
        draft.before,
        draft.after,
      ),
    );
    expect(onApplied).toHaveBeenCalledWith(
      "Updated code-review. The old SKILL.md is kept in Lore's backups.",
    );
  });

  it("discards a draft, and says why one failed", async () => {
    m.draftSkillEdit.mockRejectedValueOnce(
      "the agent's reply had no new SKILL.md",
    );
    render(<SkillCard skill={review} findings={[]} onApplied={() => {}} />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Propose an edit with Claude Code",
      }),
    );
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      "the agent's reply had no new SKILL.md",
    );

    fireEvent.click(
      screen.getByRole("button", { name: "Propose an edit with Claude Code" }),
    );
    fireEvent.click(await screen.findByRole("button", { name: "Discard" }));
    expect(screen.getByRole("heading", { name: "code-review" })).toBeTruthy();
    expect(m.applySkillEdit).not.toHaveBeenCalled();
  });

  it("offers no edit for a skill another source manages, or one never re-prompted", () => {
    const { rerender } = render(
      <SkillCard
        skill={{ ...review, editable_path: null }}
        findings={[]}
        onApplied={() => {}}
      />,
    );
    expect(
      screen.getByText(/A plugin or another source manages this skill/),
    ).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: /Propose an edit/ }),
    ).toBeNull();
    rerender(
      <SkillCard
        skill={{
          ...review,
          uses: 0,
          sessions: 0,
          last_ms: null,
          reprompts: [],
        }}
        findings={[]}
        onApplied={() => {}}
      />,
    );
    expect(screen.getByText("Never used.")).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: /Propose an edit/ }),
    ).toBeNull();
  });
});
