import { describe, expect, it } from "vitest";

import type { FileEventDto, MessageDto, MessagePartDto } from "../ipc";
import { parseDiff, patchPaths } from "./Diff";
import { readCall, readResult, summarizeRun } from "./tool";
import { buildTranscript, callsByFile, callsOf, fileByCall } from "./transcript-model";

function use(content: object, ordinal = 0): MessagePartDto {
  return { ordinal, kind: "tool_use", text: null, content_json: JSON.stringify(content), searchable: false };
}

function result(text: string, content: unknown = text, ordinal = 0): MessagePartDto {
  return { ordinal, kind: "tool_result", text, content_json: JSON.stringify(content), searchable: true };
}

function text(value: string, ordinal = 0): MessagePartDto {
  return { ordinal, kind: "text", text: value, content_json: null, searchable: true };
}

function msg(id: string, role: string, parts: MessagePartDto[], ts: number | null = null): MessageDto {
  return { id, seq: Number(id.slice(1)), role, event_kind: "message", is_sidechain: false, ts, model: null, parts };
}

describe("readCall", () => {
  it("reads a Codex apply_patch call as a patch on the file it names", () => {
    const call = readCall(
      use({ type: "custom_tool_call", name: "apply_patch", input: "*** Begin Patch\n*** Update File: a.rs\n@@\n-old\n+new\n*** End Patch" }),
      "m1",
      null,
    );
    expect([call.kind, call.argument, call.paths, call.raw]).toEqual(["edit", { text: "a.rs", isPath: true }, ["a.rs"], null]);
    expect(call.patch?.split("\n")[3]).toBe("-old");
  });

  it("reads a Codex shell call from its JSON-encoded arguments, without the shell wrapper", () => {
    const call = readCall(
      use({ type: "function_call", name: "shell", arguments: JSON.stringify({ command: ["bash", "-lc", "rg -n retry"], timeout_ms: 1000 }) }),
      "m1",
      null,
    );
    expect([call.kind, call.argument?.text, call.fields]).toEqual(["run", "rg -n retry", [["Timeout ms", "1000"]]]);
  });

  it("turns a Claude Edit into a before and after patch, keeping the path out of the fields", () => {
    const call = readCall(use({ type: "tool_use", name: "Edit", input: { file_path: "a.rs", old_string: "x = 1", new_string: "x = 2" } }), "m1", 5);
    expect(call.patch).toBe("--- a.rs\n+++ a.rs\n-x = 1\n+x = 2");
    expect([call.argument, call.fields, call.paths, call.ts]).toEqual([{ text: "a.rs", isPath: true }, [], ["a.rs"], 5]);
    expect(JSON.parse(call.raw ?? "")).toEqual({ file_path: "a.rs", old_string: "x = 1", new_string: "x = 2" });
  });
});

describe("readResult", () => {
  it("unwraps Codex exec output and reads its exit code", () => {
    const wrapped = JSON.stringify({ output: "1 failing\n", metadata: { exit_code: 1 } });
    const read = readResult(result(wrapped, { type: "function_call_output", call_id: "c1", output: wrapped }));
    expect(read).toEqual({ id: "c1", result: { text: "1 failing\n", failed: true, exitCode: 1 } });
  });

  it("does not call a result failed without the result saying so", () => {
    expect(readResult(result("permission denied")).result.failed).toBe(false);
    expect(readResult(result("x", { call_id: "c", status: "failed", output: "x" })).result).toEqual({ text: "x", failed: true, exitCode: null });
  });
});

describe("buildTranscript", () => {
  it("pairs Claude results with their calls in order and keeps one run per stretch of agent turns", () => {
    const blocks = buildTranscript([
      msg("m0", "user", [text("Why is the total off?")]),
      msg("m1", "assistant", [text("Looking."), use({ name: "Read", id: "t1", input: { file_path: "a.ts" } }, 1), use({ name: "Bash", id: "t2", input: { command: "npm test" } }, 2)]),
      msg("m2", "user", [result("const a = 1", "const a = 1", 0), result("1 passed", "1 passed", 1)]),
      msg("m3", "assistant", [text("Fixed.")]),
      msg("m4", "user", [text("Thanks")]),
    ]);
    expect(blocks.map((b) => b.kind)).toEqual(["user", "agent", "user"]);
    const run = blocks[1];
    expect(run.items.map((i) => i.kind)).toEqual(["text", "tools", "text"]);
    const calls = callsOf(blocks);
    expect(calls.map((c) => [c.name, c.result?.text])).toEqual([
      ["Read", "const a = 1"],
      ["Bash", "1 passed"],
    ]);
  });

  it("pairs Codex outputs by call id even out of order", () => {
    const blocks = buildTranscript([
      msg("m0", "assistant", [use({ type: "function_call", name: "shell", call_id: "a", arguments: "{\"command\":\"ls\"}" })]),
      msg("m1", "assistant", [use({ type: "function_call", name: "shell", call_id: "b", arguments: "{\"command\":\"pwd\"}" })]),
      msg("m2", "tool", [result("/repo", { type: "function_call_output", call_id: "b", output: "/repo" })]),
      msg("m3", "tool", [result("a.txt", { type: "function_call_output", call_id: "a", output: "a.txt" })]),
    ]);
    expect(blocks).toHaveLength(1);
    expect(callsOf(blocks).map((c) => [c.argument?.text, c.result?.text])).toEqual([
      ["ls", "a.txt"],
      ["pwd", "/repo"],
    ]);
  });

  it("shows a result with no call as its own Output row", () => {
    const calls = callsOf(buildTranscript([msg("m0", "tool", [result("orphan", { call_id: "zz", output: "orphan" })])]));
    expect(calls.map((c) => [c.name, c.result?.text])).toEqual([["Output", "orphan"]]);
  });
});

describe("summarizeRun", () => {
  it("says what a run of calls did, in order of first use", () => {
    const calls = [
      readCall(use({ name: "Read", input: { file_path: "a" } }), "m1", null),
      readCall(use({ name: "Bash", input: { command: "ls" } }), "m2", null),
      readCall(use({ name: "Read", input: { file_path: "b" } }), "m3", null),
      readCall(use({ name: "mcp__linear__get_issue", input: {} }), "m4", null),
    ];
    expect(summarizeRun(calls)).toBe("Read 2 files, ran 1 command, used mcp__linear__get_issue");
  });
});

describe("matching files to calls", () => {
  const file = (id: string, path: string): FileEventDto => ({
    id,
    path,
    change_kind: "edit",
    old_path: null,
    lines_added: 1,
    lines_removed: 0,
    source: "agent_patch",
    has_patch: true,
  });

  it("matches an absolute tool path to a relative file path, and pairs one to one only", () => {
    const calls = callsOf(
      buildTranscript([
        msg("m0", "assistant", [
          use({ name: "Edit", input: { file_path: "/repo/src/a.ts", old_string: "a", new_string: "b" } }),
          use({ name: "Read", input: { file_path: "/repo/src/b.ts" } }, 1),
          use({ name: "Edit", input: { file_path: "/repo/src/b.ts", old_string: "a", new_string: "b" } }, 2),
          use({ name: "Write", input: { file_path: "/repo/src/b.ts", content: "b" } }, 3),
        ]),
      ]),
    );
    const files = [file("fa", "src/a.ts"), file("fb", "src/b.ts")];
    const byFile = callsByFile(files, calls);
    expect([...byFile].map(([id, cs]) => [id, cs.map((c) => c.key)])).toEqual([
      ["fa", ["m0:0"]],
      ["fb", ["m0:2", "m0:3"]],
    ]);
    expect([...fileByCall(files, byFile)].map(([key, f]) => [key, f.id])).toEqual([["m0:0", "fa"]]);
  });
});

describe("patches", () => {
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

  it("names the files a patch touches", () => {
    expect(patchPaths("--- a/x.ts\n+++ b/x.ts\n@@\n*** Update File: y.rs\n+++ /dev/null")).toEqual(["x.ts", "y.rs"]);
  });
});
