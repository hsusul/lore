import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({ createTask: vi.fn() }));

import { createTask } from "../../ipc";
import NewAgentForm from "./NewAgentForm";
import { deriveTitle } from "./state";
import { task } from "./testTask";

function renderForm(overrides: Partial<Parameters<typeof NewAgentForm>[0]> = {}) {
  const props = {
    workspace: "/repo",
    onCreated: vi.fn(),
    onOpenFolder: vi.fn(),
    focusToken: 0,
    ...overrides,
  };
  const view = render(<NewAgentForm {...props} />);
  return { ...props, rerender: (next: Partial<typeof props>) => view.rerender(<NewAgentForm {...props} {...next} />) };
}

function chooseAgent(name: "Claude Code" | "Codex") {
  fireEvent.click(screen.getByLabelText("Agent"));
  fireEvent.click(screen.getByRole("menuitem", { name: /^Agent\b/ }));
  fireEvent.click(screen.getByRole("menuitemradio", { name }));
  // The picker returns to the root pane; the chosen agent is shown, menu stays open.
  expect(screen.queryByRole("menuitemradio", { name })).toBeNull();
  // Permission modes are Claude's; Codex runs in its own sandbox, so the row goes away.
  if (name === "Claude Code") expect(screen.getByRole("menuitem", { name: /^Permission\b/ })).toBeTruthy();
  else expect(screen.queryByRole("menuitem", { name: /^Permission\b/ })).toBeNull();
  expect(screen.getByRole("menuitem", { name: /^Agent\b/ }).textContent).toContain(name);
}

function openPermission() {
  fireEvent.click(screen.getByLabelText("Agent"));
  fireEvent.click(screen.getByRole("menuitem", { name: /^Permission\b/ }));
}

beforeEach(() => {
  vi.mocked(createTask).mockReset();
});

describe("deriveTitle", () => {
  it("takes the first line without list or heading markers", () => {
    expect(deriveTitle("\n  - Fix the flaky parser test\nthen run it")).toBe("Fix the flaky parser test");
    expect(deriveTitle("## Upgrade to Vite 6")).toBe("Upgrade to Vite 6");
    expect(deriveTitle("1. Add docs")).toBe("Add docs");
    expect(deriveTitle("   ")).toBe("");
  });

  it("cuts long lines at a word boundary with an ellipsis", () => {
    const title = deriveTitle(
      "Refactor the pricing service so that tax is computed per line item instead of per cart",
    );
    expect(title.length).toBeLessThanOrEqual(60);
    expect(title.endsWith("…")).toBe(true);
    expect(title).toBe("Refactor the pricing service so that tax is computed per…");
  });
});

describe("NewAgentForm", () => {
  it("launches with the workspace root as the repository (⌘Enter submits)", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ id: "new" }));
    const props = renderForm();

    fireEvent.change(screen.getByLabelText("Title"), { target: { value: " Fix parser " } });
    chooseAgent("Codex");
    const prompt = screen.getByLabelText("Prompt");
    fireEvent.change(prompt, { target: { value: "fix it" } });
    fireEvent.keyDown(prompt, { key: "Enter", metaKey: true });

    await waitFor(() =>
      expect(createTask).toHaveBeenCalledWith({
        repo_path: "/repo",
        title: "Fix parser",
        prompt: "fix it",
        agent: "codex",
        permission: "edits",
        auto_handoff: true,
      }),
    );
    await waitFor(() => expect(props.onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "new" })));
  });


  it("sends the chosen permission, defaulting to Edits", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ id: "new" }));
    renderForm();
    openPermission();
    const group = screen.getByRole("radiogroup", { name: "Permission" });
    const edits = within(group).getByRole("radio", { name: "Edits" });
    const auto = within(group).getByRole("radio", { name: "Auto" });
    expect(edits.getAttribute("aria-checked")).toBe("true");
    expect(edits.getAttribute("title")).toBe("Claude may edit files but not run shell commands.");
    expect(auto.getAttribute("title")).toMatch(/safety classifier/);

    fireEvent.click(auto);
    // The picker returns to the root pane; Auto is the shown permission.
    expect(screen.queryByRole("radiogroup", { name: "Permission" })).toBeNull();
    expect(screen.getByRole("menuitem", { name: /^Permission\b/ }).textContent).toContain("Auto");
    fireEvent.click(screen.getByRole("menuitem", { name: /^Model\b/ }));
    fireEvent.click(screen.getByRole("radio", { name: "Opus" }));
    expect(screen.getByRole("menuitem", { name: /^Model\b/ }).textContent).toContain("Opus");
    fireEvent.click(screen.getByRole("menuitem", { name: /^Effort\b/ }));
    fireEvent.click(screen.getByRole("radio", { name: "High" }));
    expect(screen.getByRole("menuitem", { name: /^Effort\b/ }).textContent).toContain("High");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "T" } });
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "P" } });
    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    await waitFor(() =>
      expect(createTask).toHaveBeenCalledWith(
        expect.objectContaining({ permission: "auto", model: "opus", effort: "high" }),
      ),
    );
  });


  it("disables the form with a hint when no workspace is open", () => {
    const props = renderForm({ workspace: null });
    expect(screen.getByRole("button", { name: "Launch" }).matches(":disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Open folder…" }));
    expect(props.onOpenFolder).toHaveBeenCalled();
  });


  it("shows backend errors inline", async () => {
    vi.mocked(createTask).mockRejectedValue("not a git repository: /repo");
    renderForm();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "T" } });
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "P" } });
    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    expect((await screen.findByRole("alert")).textContent).toBe("not a git repository: /repo");
  });


  it("parses claims into chips and sends them with auto-handoff", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ id: "new" }));
    renderForm();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "T" } });
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "P" } });
    fireEvent.change(screen.getByLabelText("Owns (files or folders)"), {
      target: { value: " src/parser/ , docs/SCHEMA.md\n./src/parser/ \n" },
    });

    // Comma- and newline-separated, trimmed, deduplicated, shown as chips.
    const chips = within(screen.getByRole("list", { name: "Owned paths" })).getAllByRole("listitem");
    expect(chips.map((li) => li.textContent)).toEqual(["src/parser/", "docs/SCHEMA.md"]);
    expect(screen.getByText(/Other agents are told not to touch these/)).toBeTruthy();

    fireEvent.click(screen.getByLabelText("Agent"));
    const autoHandoff = screen.getByLabelText("Auto-handoff on usage limit");
    expect(autoHandoff.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(autoHandoff);

    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    await waitFor(() =>
      expect(createTask).toHaveBeenCalledWith(
        expect.objectContaining({ claims: ["src/parser/", "docs/SCHEMA.md"], auto_handoff: false }),
      ),
    );
  });


  it("omits claims entirely when the field is empty", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ id: "new" }));
    renderForm();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "T" } });
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "P" } });
    fireEvent.change(screen.getByLabelText("Owns (files or folders)"), { target: { value: " , \n " } });
    expect(screen.queryByRole("list", { name: "Owned paths" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    await waitFor(() => expect(createTask).toHaveBeenCalled());
    expect(vi.mocked(createTask).mock.calls[0][0]).not.toHaveProperty("claims");
  });


  it("titles the task from the prompt when the title is left empty", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ id: "new" }));
    renderForm();
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "Make checkout retry on 503\nwith backoff" } });
    expect(screen.getByLabelText("Title").getAttribute("placeholder")).toBe("Title: Make checkout retry on 503");
    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    await waitFor(() =>
      expect(createTask).toHaveBeenCalledWith(
        expect.objectContaining({ title: "Make checkout retry on 503", prompt: "Make checkout retry on 503\nwith backoff" }),
      ),
    );
  });

  it("asks for a prompt instead of sending an empty one", () => {
    renderForm();
    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    expect(screen.getByRole("alert").textContent).toBe("Describe what the agent should do.");
    expect(createTask).not.toHaveBeenCalled();
  });

  it("focuses the prompt and applies a draft when asked", () => {
    const view = renderForm();
    view.rerender({ focusToken: 1, draft: "Add a dark mode toggle" });
    const prompt = screen.getByLabelText("Prompt") as HTMLTextAreaElement;
    expect(document.activeElement).toBe(prompt);
    expect(prompt.value).toBe("Add a dark mode toggle");
  });
});
