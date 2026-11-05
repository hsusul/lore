// @vitest-environment node
import { describe, expect, it } from "vitest";

import { shownTurn } from "./turns";

describe("shownTurn", () => {
  it("writes a slash command as the person typed it", () => {
    expect(
      shownTurn("<command-message>poteto-mode</command-message>\n<command-name>/poteto-mode</command-name>\n<command-args>what else did paul want</command-args>"),
    ).toEqual({ text: "/poteto-mode what else did paul want", quiet: false });
    expect(shownTurn("<command-name>review</command-name>")).toEqual({ text: "/review", quiet: false });
  });

  it("turns an injected skill into one quiet line", () => {
    expect(shownTurn("Base directory for this skill: /Users/henry/.claude/skills/poteto-mode\n\n# Poteto mode\n...")).toEqual({
      text: "Loaded the poteto-mode skill",
      quiet: true,
    });
  });

  it("leaves out system reminders, and a turn that was only one", () => {
    expect(shownTurn("fix it <system-reminder>be brief</system-reminder>")).toEqual({ text: "fix it", quiet: false });
    expect(shownTurn("<system-reminder>be brief</system-reminder>")).toBeNull();
    expect(shownTurn("plain words")).toEqual({ text: "plain words", quiet: false });
  });
});
