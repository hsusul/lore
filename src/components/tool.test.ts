import { describe, expect, it } from "vitest";

import { parseDiff } from "./Diff";
import { describeTool } from "./tool";

describe("describeTool", () => {
  it("reads a Codex apply_patch call as a patch", () => {
    const view = describeTool(
      JSON.stringify({ type: "custom_tool_call", name: "apply_patch", input: "*** Begin Patch\n*** Update File: a.rs\n@@\n-old\n+new\n*** End Patch" }),
      null,
    );
    expect(view.name).toBe("apply_patch");
    expect(view.detail).toBe("*** Begin Patch");
    expect(view.patch?.split("\n")[3]).toBe("-old");
  });

  it("reads a Codex shell call from its JSON-encoded arguments", () => {
    const view = describeTool(
      JSON.stringify({ type: "function_call", name: "shell", arguments: JSON.stringify({ command: ["rg", "-n", "retry"] }) }),
      null,
    );
    expect([view.name, view.detail, view.patch]).toEqual(["shell", "rg -n retry", null]);
  });

  it("turns a Claude Edit into a before and after patch", () => {
    const view = describeTool(
      JSON.stringify({ type: "tool_use", name: "Edit", input: { file_path: "a.rs", old_string: "x = 1", new_string: "x = 2" } }),
      null,
    );
    expect(view.patch).toBe("--- a.rs\n+++ a.rs\n-x = 1\n+x = 2");
  });

  it("flags a failed tool result", () => {
    const view = describeTool(JSON.stringify({ is_error: true }), "permission denied");
    expect([view.isError, view.detail, view.body]).toEqual([true, "permission denied", "permission denied"]);
  });
});

describe("parseDiff", () => {
  it("numbers old and new lines through a hunk", () => {
    const lines = parseDiff("--- a/x\n+++ b/x\n@@ -10,3 +10,3 @@\n keep\n-gone\n+came\n--- was a comment\n");
    expect(lines.map((l) => [l.kind, l.oldNo, l.newNo])).toEqual([
      ["file", null, null],
      ["file", null, null],
      ["hunk", null, null],
      ["context", 10, 10],
      ["del", 11, null],
      ["add", null, 11],
      ["del", 12, null],
    ]);
  });
});
