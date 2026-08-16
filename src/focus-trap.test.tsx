import { fireEvent, render } from "@testing-library/react";
import { useRef } from "react";
import { describe, expect, it } from "vitest";

import { useFocusTrap } from "./focus-trap";

function TrapComponent({
  active = true,
  children,
}: {
  active?: boolean;
  children?: React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useFocusTrap(ref, active);
  return (
    <div ref={ref} data-testid="trap-container">
      {children}
    </div>
  );
}

describe("useFocusTrap", () => {
  it("traps forward Tab navigation from last to first", () => {
    const { getByTestId, getByText } = render(
      <TrapComponent>
        <button type="button">First</button>
        <button type="button">Second</button>
      </TrapComponent>,
    );
    const container = getByTestId("trap-container");
    const first = getByText("First");
    const second = getByText("Second");

    second.focus();
    expect(document.activeElement).toBe(second);

    fireEvent.keyDown(container, { key: "Tab" });
    expect(document.activeElement).toBe(first);
  });

  it("traps backward Shift+Tab navigation from first to last", () => {
    const { getByTestId, getByText } = render(
      <TrapComponent>
        <button type="button">First</button>
        <button type="button">Second</button>
      </TrapComponent>,
    );
    const container = getByTestId("trap-container");
    const first = getByText("First");
    const second = getByText("Second");

    first.focus();
    expect(document.activeElement).toBe(first);

    fireEvent.keyDown(container, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(second);
  });

  it("ignores aria-hidden and disabled elements", () => {
    const { getByTestId, getByText } = render(
      <TrapComponent>
        <button type="button">Active 1</button>
        <button type="button" disabled>
          Disabled
        </button>
        <button type="button" aria-hidden="true">
          Hidden
        </button>
        <button type="button">Active 2</button>
      </TrapComponent>,
    );
    const container = getByTestId("trap-container");
    const active1 = getByText("Active 1");
    const active2 = getByText("Active 2");

    active2.focus();
    fireEvent.keyDown(container, { key: "Tab" });
    expect(document.activeElement).toBe(active1);
  });

  it("prevents Tab when no focusable elements are present", () => {
    const { getByTestId } = render(
      <TrapComponent>
        <p>No interactive elements</p>
      </TrapComponent>,
    );
    const container = getByTestId("trap-container");
    const event = new KeyboardEvent("keydown", { key: "Tab", cancelable: true, bubbles: true });
    container.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });
});
