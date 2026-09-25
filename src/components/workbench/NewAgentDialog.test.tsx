import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({ createTask: vi.fn() }));

import { createTask } from "../../ipc";
import NewAgentDialog from "./NewAgentDialog";
import { task } from "./testTask";

function renderDialog(overrides: Partial<Parameters<typeof NewAgentDialog>[0]> = {}) {
  const props = {
    workspace: "/work/acme",
    baseBranch: "main",
    onClose: vi.fn(),
    onCreated: vi.fn(),
    onOpenFolder: vi.fn(),
    ...overrides,
  };
  const view = render(<NewAgentDialog {...props} />);
  return { props, view };
}

beforeEach(() => {
  vi.mocked(createTask).mockReset();
});

describe("NewAgentDialog", () => {
  it("is a labelled modal with the repository and branch, focused on the prompt", () => {
    renderDialog({ draft: "Add a dark mode toggle" });
    const dialog = screen.getByRole("dialog", { name: "New agent" });
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.textContent).toContain("acme");
    expect(dialog.textContent).toContain("main");
    const prompt = screen.getByLabelText("Prompt") as HTMLTextAreaElement;
    expect(document.activeElement).toBe(prompt);
    expect(prompt.value).toBe("Add a dark mode toggle");
  });

  it("closes on Escape, the close button, or a click outside, and gives focus back", () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const { props, view } = renderDialog();
    fireEvent.keyDown(screen.getByLabelText("Prompt"), { key: "Escape" });
    expect(props.onClose).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(props.onClose).toHaveBeenCalledTimes(2);
    // Inside the dialog is not outside.
    fireEvent.mouseDown(screen.getByRole("dialog"));
    expect(props.onClose).toHaveBeenCalledTimes(2);
    fireEvent.mouseDown(screen.getByRole("dialog").parentElement!);
    expect(props.onClose).toHaveBeenCalledTimes(3);
    view.unmount();
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  it("keeps Tab inside the dialog", () => {
    renderDialog();
    const dialog = screen.getByRole("dialog");
    const close = screen.getByRole("button", { name: "Close" });
    const launch = screen.getByRole("button", { name: "Launch" });
    launch.focus();
    fireEvent.keyDown(launch, { key: "Tab" });
    expect(document.activeElement).toBe(close);
    fireEvent.keyDown(close, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(launch);
    expect(dialog.contains(document.activeElement)).toBe(true);
  });

  it("launches in the repository and hands the new task back", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ id: "new" }));
    const { props } = renderDialog();
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "Fix the flaky parser test" } });
    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    await waitFor(() => expect(props.onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "new" })));
    expect(createTask).toHaveBeenCalledWith(
      expect.objectContaining({ repo_path: "/work/acme", title: "Fix the flaky parser test" }),
    );
  });
});
