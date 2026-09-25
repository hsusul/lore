import { describe, expect, it } from "vitest";

import { eventHeadline, taskEvents } from "./notify";
import { task } from "./testTask";

describe("taskEvents", () => {
  it("stays quiet on the first load", () => {
    expect(taskEvents(null, [task({ state: "failed" })])).toEqual([]);
  });

  it("reports finishing, failing, needing the user, merging, and handing off", () => {
    const before = [
      task({ id: "a", title: "A" }),
      task({ id: "b", title: "B" }),
      task({ id: "c", title: "C" }),
      task({ id: "d", title: "D", state: "finished" }),
      task({ id: "e", title: "E" }),
      task({ id: "f", title: "F" }),
    ];
    const after = [
      task({ id: "a", title: "A", state: "finished", last_activity: "All green" }),
      task({ id: "b", title: "B", state: "failed", exit_code: 3 }),
      task({ id: "c", title: "C", state: "failed", attention: "Usage limit reached" }),
      task({ id: "d", title: "D", state: "finished", merged_into: "main" }),
      task({ id: "e", title: "E", agent: "codex" }),
      task({ id: "f", title: "F" }),
      task({ id: "new", title: "New" }),
    ];
    const events = taskEvents(before, after);
    expect(events.map((e) => [e.id, e.kind, e.message])).toEqual([
      ["a", "finished", "All green"],
      ["b", "failed", "The agent exited with code 3."],
      ["c", "attention", "Usage limit reached"],
      ["d", "merged", "Merged into main"],
      ["e", "handoff", "Handed off to Codex."],
    ]);
    expect(events.map(eventHeadline)).toEqual([
      "A finished",
      "B failed",
      "C needs you",
      "D merged",
      "E was handed off",
    ]);
  });
});
