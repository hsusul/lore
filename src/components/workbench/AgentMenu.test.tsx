import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";

import type { TaskAgent, TaskEffort, TaskPermission } from "../../ipc";
import AgentMenu from "./AgentMenu";

function Harness({ initialAgent = "claude_code" }: { initialAgent?: TaskAgent }) {
  const [agent, setAgent] = useState<TaskAgent>(initialAgent);
  const [permission, setPermission] = useState<TaskPermission>("edits");
  const [autoHandoff, setAutoHandoff] = useState(true);
  const [model, setModel] = useState<string | null>(null);
  const [effort, setEffort] = useState<TaskEffort | null>(null);
  return (
    <>
      <AgentMenu
        label="Agent"
        agent={agent}
        onAgentChange={setAgent}
        permission={permission}
        onPermissionChange={setPermission}
        autoHandoff={autoHandoff}
        onAutoHandoffChange={setAutoHandoff}
        model={model}
        onModelChange={setModel}
        effort={effort}
        onEffortChange={setEffort}
      />
      <button type="button">After</button>
    </>
  );
}

const key = (k: string) => fireEvent.keyDown(document.activeElement!, { key: k });

describe("AgentMenu keyboard", () => {
  it("moves focus in on open, drills in with → and back with ←, and returns focus on Escape", () => {
    render(<Harness />);
    const trigger = screen.getByLabelText("Agent");
    fireEvent.click(trigger);
    expect(document.activeElement).toBe(screen.getByLabelText("Auto-handoff on usage limit"));

    key("ArrowDown");
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: /^Permission\b/ }));
    key("ArrowRight");
    // The current choice takes focus in the sub-pane.
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "Edits" }));
    key("ArrowDown");
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "Auto" }));
    key("ArrowLeft");
    // Back on the root pane, on the row that opened the sub-pane.
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: /^Permission\b/ }));

    key("End");
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: /^Effort\b/ }));
    key("ArrowDown");
    expect(document.activeElement).toBe(screen.getByLabelText("Auto-handoff on usage limit"));

    key("Escape");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("goes back with Escape or the Back row before closing, and lands on the row it came from", () => {
    render(<Harness />);
    fireEvent.click(screen.getByLabelText("Agent"));
    fireEvent.click(screen.getByRole("menuitem", { name: /^Model\b/ }));
    expect(screen.getByRole("radiogroup", { name: "Model" })).toBeTruthy();
    key("Escape");
    expect(screen.queryByRole("radiogroup", { name: "Model" })).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: /^Model\b/ }));

    fireEvent.click(screen.getByRole("menuitem", { name: /^Effort\b/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Back from Effort" }));
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: /^Effort\b/ }));
  });

  it("shows option hints as visible text tied to each option", () => {
    render(<Harness />);
    fireEvent.click(screen.getByLabelText("Agent"));
    fireEvent.click(screen.getByRole("menuitem", { name: /^Permission\b/ }));
    const auto = screen.getByRole("radio", { name: "Auto" });
    const hint = document.getElementById(auto.getAttribute("aria-describedby")!);
    expect(hint?.textContent).toMatch(/safety classifier/);
  });

  it("hides the Claude-only permission row for Codex", () => {
    render(<Harness initialAgent="codex" />);
    fireEvent.click(screen.getByLabelText("Agent"));
    expect(screen.queryByRole("menuitem", { name: /^Permission\b/ })).toBeNull();
    expect(screen.getByLabelText("Auto-handoff on usage limit")).toBeTruthy();
  });

  it("opens from the keyboard with ↓ and closes on Tab back to the trigger", () => {
    render(<Harness />);
    const trigger = screen.getByLabelText("Agent");
    trigger.focus();
    key("ArrowDown");
    expect(screen.getByRole("menu")).toBeTruthy();
    key("Tab");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
});
