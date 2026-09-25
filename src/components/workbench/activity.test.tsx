import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { ActivityDto } from "../../ipc";
import { buildTimeline, openablePath, parseTool } from "./activity";
import ActivityList, { OUTPUT_PREVIEW_LINES } from "./ActivityList";

const out = (text: string): ActivityDto => ({ kind: "output", text });

describe("buildTimeline", () => {
  it("shows each run with the prompt that started it, run 1 without a header in the log", () => {
    const timeline = buildTimeline([
      out("--- Lore: opening prompt ---"),
      out("Fix the parser"),
      out("and add a test"),
      { kind: "message", text: "On it." },
      { kind: "tool", text: "Edit src/a.rs" },
      { kind: "result", text: "Done." },
      out("--- Lore: opening prompt ---"),
      out("The previous agent stopped because it hit its usage limit."),
      out("--- Lore: run 2 (Codex) ---"),
      { kind: "tool", text: "run cargo test" },
    ]);
    expect(timeline).toEqual([
      { kind: "run", run: 1, agent: null },
      { kind: "prompt", text: "Fix the parser\nand add a test" },
      { kind: "message", text: "On it." },
      { kind: "tool", tool: { name: "Edit", target: "src/a.rs", family: "edit" } },
      { kind: "result", text: "Done." },
      { kind: "run", run: 2, agent: "Codex" },
      { kind: "prompt", text: "The previous agent stopped because it hit its usage limit." },
      { kind: "tool", tool: { name: "run", target: "cargo test", family: "shell" } },
    ]);
  });

  it("keeps a prompt whose error-looking line the backend classed as an error", () => {
    const timeline = buildTimeline([
      out("--- Lore: opening prompt ---"),
      { kind: "error", text: "Fix the error in parse()" },
    ]);
    expect(timeline).toEqual([
      { kind: "run", run: 1, agent: null },
      { kind: "prompt", text: "Fix the error in parse()" },
    ]);
  });

  it("groups consecutive output lines and passes a log without markers through", () => {
    const timeline = buildTimeline([out("a"), out("b"), { kind: "error", text: "boom" }, out("c")]);
    expect(timeline).toEqual([
      { kind: "output", lines: ["a", "b"] },
      { kind: "error", text: "boom" },
      { kind: "output", lines: ["c"] },
    ]);
  });
});

describe("parseTool / openablePath", () => {
  it("names the tool family and only opens relative read/edit paths", () => {
    expect(parseTool("Read src/a.rs")).toEqual({ name: "Read", target: "src/a.rs", family: "read" });
    expect(parseTool("edit files")).toEqual({ name: "Edit", target: "files", family: "edit" });
    expect(parseTool("Grep \"tax_bps\" services/").family).toBe("search");
    expect(parseTool("shell: rg -n x").family).toBe("shell");
    expect(parseTool("TodoWrite").family).toBe("other");
    expect(openablePath(parseTool("Edit src/a.rs"))).toBe("src/a.rs");
    expect(openablePath(parseTool("Read /etc/hosts"))).toBeNull();
    expect(openablePath(parseTool("Edit ../outside.rs"))).toBeNull();
    expect(openablePath(parseTool("Bash npm test"))).toBeNull();
  });
});

describe("ActivityList", () => {
  it("renders messages as Markdown with links that cannot navigate the app", () => {
    render(
      <ActivityList
        label="Activity"
        error={null}
        items={[{ kind: "message", text: "**Plan:** see [the docs](https://example.com)\n\n- one\n- two" }]}
      />,
    );
    const list = screen.getByRole("list", { name: "Activity" });
    expect(within(list).getByText("Plan:").tagName).toBe("STRONG");
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toContain("one");
    expect(list.querySelector("a")).toBeNull();
    expect(within(list).getByText("the docs").getAttribute("title")).toBe("https://example.com");
  });

  it("collapses long output to its last lines", () => {
    const lines = Array.from({ length: OUTPUT_PREVIEW_LINES + 4 }, (_, i) => out(`line ${i + 1}`));
    const { container } = render(<ActivityList label="Activity" error={null} items={lines} />);
    const pre = () => container.querySelector("pre.activity__output")!.textContent!.split("\n");
    expect(pre()).toHaveLength(OUTPUT_PREVIEW_LINES);
    expect(pre()[0]).toBe("line 5");
    const more = screen.getByRole("button", { name: "Show 4 earlier lines" });
    fireEvent.click(more);
    expect(more.getAttribute("aria-expanded")).toBe("true");
    expect(pre()[0]).toBe("line 1");
  });

  it("opens files the agent read or edited, and shows that a running agent is working", () => {
    const onOpenFile = vi.fn();
    render(
      <ActivityList
        label="Activity"
        error={null}
        running
        onOpenFile={onOpenFile}
        items={[{ kind: "tool", text: "Edit src/lib/money.ts" }, { kind: "tool", text: "Bash npm test" }]}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "src/lib/money.ts" }));
    expect(onOpenFile).toHaveBeenCalledWith("src/lib/money.ts");
    expect(screen.queryByRole("button", { name: "npm test" })).toBeNull();
    expect(screen.getByRole("status").textContent).toContain("Working…");
  });
});
