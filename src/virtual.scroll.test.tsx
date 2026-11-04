import { fireEvent, render } from "@testing-library/react";
import { useEffect, useRef } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useWindowing } from "./virtual";

// The effect ListBox runs: bring the open row into view when it changes.
function List({ count, open }: { count: number; open: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const { scrollToIndex } = useWindowing(ref, count, { estimateRowHeight: 40 });
  useEffect(() => {
    if (open >= 0) scrollToIndex(open);
  }, [open, scrollToIndex]);
  return <div ref={ref} data-testid="list" />;
}

describe("useWindowing scrollToIndex", () => {
  const clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
  beforeEach(() => {
    Object.defineProperty(HTMLElement.prototype, "clientHeight", {
      configurable: true,
      get: () => 200,
    });
  });
  afterEach(() => {
    if (clientHeight) Object.defineProperty(HTMLElement.prototype, "clientHeight", clientHeight);
  });

  it("leaves the user's scroll alone when more rows load", () => {
    const { getByTestId, rerender } = render(<List count={50} open={0} />);
    const list = getByTestId("list");
    list.scrollTop = 1000;
    fireEvent.scroll(list);

    rerender(<List count={100} open={0} />);

    expect(list.scrollTop).toBe(1000);
  });

  it("still brings a newly opened row into view", () => {
    const { getByTestId, rerender } = render(<List count={50} open={0} />);
    const list = getByTestId("list");

    rerender(<List count={50} open={20} />);

    expect(list.scrollTop).toBe(21 * 40 - 200);
  });
});
