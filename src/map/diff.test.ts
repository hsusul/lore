// @vitest-environment node
import { describe, expect, it } from "vitest";

import { hunks, lineDiff } from "./diff";

describe("lineDiff", () => {
  it("keeps common lines and marks what was removed and added", () => {
    expect(lineDiff("a\nb\nc", "a\nB\nc\nd")).toEqual([
      { op: "=", text: "a" },
      { op: "-", text: "b" },
      { op: "+", text: "B" },
      { op: "=", text: "c" },
      { op: "+", text: "d" },
    ]);
    expect(lineDiff("same", "same")).toEqual([{ op: "=", text: "same" }]);
  });

  it("shows only the changes and a line around each, with gaps between", () => {
    const lines = lineDiff("1\n2\n3\n4\n5\n6\n7", "1\n2\nthree\n4\n5\n6\nseven");
    expect(hunks(lines)).toEqual([
      { op: "=", text: "2" },
      { op: "-", text: "3" },
      { op: "+", text: "three" },
      { op: "=", text: "4" },
      null,
      { op: "=", text: "6" },
      { op: "-", text: "7" },
      { op: "+", text: "seven" },
    ]);
    expect(hunks(lineDiff("a", "a"))).toEqual([]);
  });
});
