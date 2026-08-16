import { describe, expect, it } from "vitest";

import { computeWindow } from "./virtual";

describe("computeWindow", () => {
  it("renders every row when nothing can be measured (fallback)", () => {
    // viewportHeight 0 is the jsdom / pre-layout case — degrade to a full render.
    expect(computeWindow(1000, 0, 0, 0, 8)).toEqual({
      startIndex: 0,
      endIndex: 1000,
      padTop: 0,
      padBottom: 0,
    });
  });

  it("windows to the visible slice plus overscan", () => {
    // 1000 rows of 40px, 400px viewport (10 visible), scrolled to row 50.
    const w = computeWindow(1000, 2000, 400, 40, 5);
    expect(w.startIndex).toBe(45); // floor(2000/40) - 5
    expect(w.endIndex).toBe(65); // ceil(2400/40) + 5
    // Spacers reserve the exact scroll extent of the off-screen rows.
    expect(w.padTop).toBe(45 * 40);
    expect(w.padBottom).toBe((1000 - 65) * 40);
    // Total reserved height equals the full list height.
    const rendered = (w.endIndex - w.startIndex) * 40;
    expect(w.padTop + rendered + w.padBottom).toBe(1000 * 40);
  });

  it("clamps at the top and bottom edges", () => {
    const top = computeWindow(1000, 0, 400, 40, 8);
    expect(top.startIndex).toBe(0);
    expect(top.padTop).toBe(0);

    // Scrolled to the very bottom: endIndex clamps to count, padBottom is 0.
    const bottom = computeWindow(1000, 40 * 1000 - 400, 400, 40, 8);
    expect(bottom.endIndex).toBe(1000);
    expect(bottom.padBottom).toBe(0);
  });

  it("handles negative scrollTop from rubber-band bounce", () => {
    // Negative scroll position during bounce should behave like scrollTop = 0.
    const bounce = computeWindow(1000, -50, 400, 40, 2);
    expect(bounce.startIndex).toBe(0);
    expect(bounce.endIndex).toBe(12); // ceil(400/40) + 2
    expect(bounce.padTop).toBe(0);
  });

  it("handles an empty list", () => {
    expect(computeWindow(0, 0, 400, 40, 8)).toEqual({
      startIndex: 0,
      endIndex: 0,
      padTop: 0,
      padBottom: 0,
    });
  });

  it("safely handles negative count and negative overscan", () => {
    expect(computeWindow(-5, 0, 400, 40, -2)).toEqual({
      startIndex: 0,
      endIndex: 0,
      padTop: 0,
      padBottom: 0,
    });

    const nonNegativeOverscan = computeWindow(100, 400, 400, 40, -3);
    expect(nonNegativeOverscan.startIndex).toBe(10);
    expect(nonNegativeOverscan.endIndex).toBe(20);
  });

  it("handles NaN and non-finite dimensions safely", () => {
    expect(computeWindow(NaN, 0, 400, 40, 5)).toEqual({
      startIndex: 0,
      endIndex: 0,
      padTop: 0,
      padBottom: 0,
    });
    expect(computeWindow(100, 0, NaN, 40, 5)).toEqual({
      startIndex: 0,
      endIndex: 100,
      padTop: 0,
      padBottom: 0,
    });
    expect(computeWindow(100, 0, 400, NaN, 5)).toEqual({
      startIndex: 0,
      endIndex: 100,
      padTop: 0,
      padBottom: 0,
    });
    expect(computeWindow(100, NaN, 400, 40, NaN)).toEqual({
      startIndex: 0,
      endIndex: 10,
      padTop: 0,
      padBottom: 90 * 40,
    });
  });
});
