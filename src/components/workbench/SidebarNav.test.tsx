import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import SidebarNav from "./SidebarNav";

function renderNav(overrides: Partial<Parameters<typeof SidebarNav>[0]> = {}) {
  const props = {
    workspace: "/work/acme",
    onOpenFolder: vi.fn(),
    onNewAgent: vi.fn(),
    onShowOverview: vi.fn(),
    overviewActive: true,
    attentionCount: 2,
    ...overrides,
  };
  render(<SidebarNav {...props} />);
  return props;
}

describe("SidebarNav", () => {
  it("switches repository, starts an agent, and shows the overview with what needs you", () => {
    const props = renderNav();
    fireEvent.click(screen.getByRole("button", { name: "acme" }));
    expect(props.onOpenFolder).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /New agent/ }));
    expect(props.onNewAgent).toHaveBeenCalled();
    const overview = screen.getByRole("button", { name: /Overview/ });
    expect(overview.getAttribute("aria-current")).toBe("true");
    expect(screen.getByTitle("2 need you").textContent).toBe("2");
    fireEvent.click(overview);
    expect(props.onShowOverview).toHaveBeenCalled();
  });

  it("asks for a folder first when none is open", () => {
    const props = renderNav({ workspace: null, attentionCount: 0 });
    expect(screen.getByRole("button", { name: /New agent/ }).matches(":disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Open folder…" }));
    expect(props.onOpenFolder).toHaveBeenCalled();
  });
});
