import { useCallback, useLayoutEffect, useState, type RefObject } from "react";

/**
 * Fixed-height list windowing. Kept dependency-free and split into a pure core
 * (`computeWindow`) plus a thin DOM hook (`useWindowing`) so the range math is
 * unit-testable without a browser. When the viewport or row height cannot be
 * measured (e.g. jsdom, or before first layout), it degrades to rendering every
 * row — identical to a plain `.map()` — so it can never regress a working list.
 */

export interface WindowRange {
  /** First item index to render (inclusive). */
  startIndex: number;
  /** Last item index to render (exclusive). */
  endIndex: number;
  /** Spacer height above the rendered rows, in px. */
  padTop: number;
  /** Spacer height below the rendered rows, in px. */
  padBottom: number;
}

/**
 * Pure window computation. Returns the visible slice plus spacer heights that
 * reserve the scroll extent of the off-screen rows. Falls back to the full range
 * when it has nothing to measure against.
 */
export function computeWindow(
  count: number,
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  overscan: number,
): WindowRange {
  const safeCount = Math.max(0, count);
  if (safeCount === 0 || viewportHeight <= 0 || rowHeight <= 0) {
    return { startIndex: 0, endIndex: safeCount, padTop: 0, padBottom: 0 };
  }
  const safeScrollTop = Math.max(0, scrollTop);
  const safeOverscan = Math.max(0, overscan);
  const first = Math.max(0, Math.floor(safeScrollTop / rowHeight) - safeOverscan);
  const last = Math.min(safeCount, Math.ceil((safeScrollTop + viewportHeight) / rowHeight) + safeOverscan);
  return {
    startIndex: first,
    endIndex: Math.max(first, last),
    padTop: first * rowHeight,
    padBottom: Math.max(0, (safeCount - last) * rowHeight),
  };
}

/** Options for {@link useWindowing}. */
export interface WindowingOptions {
  /** Rows to render beyond the viewport on each side, absorbing fast scrolls. */
  overscan?: number;
  /** Height (px) to assume until a real row can be measured. */
  estimateRowHeight?: number;
}

/** Result of {@link useWindowing}: the range to render plus a scroll helper. */
export interface Windowing extends WindowRange {
  /** Measured (or estimated) per-row height in px. */
  rowHeight: number;
  /** Scroll the container so item `index` is fully visible. */
  scrollToIndex: (index: number) => void;
}

/**
 * Window a scrollable list. `containerRef` must point at the element that owns
 * the scrollbar and carries rows tagged `data-vrow` (used to measure row
 * height). Re-measures on scroll, on resize, and when `count` changes.
 */
export function useWindowing(
  containerRef: RefObject<HTMLElement | null>,
  count: number,
  { overscan = 8, estimateRowHeight = 44 }: WindowingOptions = {},
): Windowing {
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(0);
  const [rowHeight, setRowHeight] = useState(estimateRowHeight);

  useLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const measure = () => {
      setViewportHeight(el.clientHeight);
      const row = el.querySelector<HTMLElement>("[data-vrow]");
      if (row && row.offsetHeight > 0) setRowHeight(row.offsetHeight);
    };
    const onScroll = () => setScrollTop(el.scrollTop);

    measure();
    el.addEventListener("scroll", onScroll, { passive: true });
    // ResizeObserver is absent in some test environments; degrade gracefully.
    const ro =
      typeof ResizeObserver !== "undefined" ? new ResizeObserver(measure) : undefined;
    ro?.observe(el);
    return () => {
      el.removeEventListener("scroll", onScroll);
      ro?.disconnect();
    };
  }, [containerRef, count]);

  const range = computeWindow(count, scrollTop, viewportHeight, rowHeight, overscan);

  // Stable across renders so a caller's `[active, scrollToIndex]` effect fires
  // only when the active row changes — not on every scroll, which would fight
  // the user's own scrolling by yanking the view back to the active row.
  const scrollToIndex = useCallback(
    (index: number) => {
      const el = containerRef.current;
      if (!el || viewportHeight <= 0 || rowHeight <= 0 || count <= 0 || index < 0) return;
      const targetIndex = Math.min(index, count - 1);
      const top = targetIndex * rowHeight;
      const bottom = top + rowHeight;
      if (top < el.scrollTop) el.scrollTop = top;
      else if (bottom > el.scrollTop + el.clientHeight) el.scrollTop = bottom - el.clientHeight;
    },
    [containerRef, count, viewportHeight, rowHeight],
  );

  return { ...range, rowHeight, scrollToIndex };
}
