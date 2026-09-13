import { useRef, type KeyboardEvent, type PointerEvent } from "react";

import { clamp } from "../../format";

type Props = {
  /** "vertical" sits between side-by-side panes; "horizontal" between stacked panes. */
  orientation: "vertical" | "horizontal";
  value: number;
  min: number;
  max: number;
  /** Pixels per pointer pixel: +1 grows rightward/downward, -1 grows leftward/upward. */
  direction: 1 | -1;
  label: string;
  onChange: (value: number) => void;
};

/** A draggable, keyboard-adjustable splitter (ARIA window splitter). */
export default function Sash({ orientation, value, min, max, direction, label, onChange }: Props) {
  const drag = useRef<{ start: number; value: number } | null>(null);
  const axis = (e: PointerEvent) => (orientation === "vertical" ? e.clientX : e.clientY);

  function onKeyDown(e: KeyboardEvent) {
    const grow = orientation === "vertical" ? "ArrowRight" : "ArrowDown";
    const shrink = orientation === "vertical" ? "ArrowLeft" : "ArrowUp";
    let delta = 0;
    if (e.key === grow) delta = 10 * direction;
    else if (e.key === shrink) delta = -10 * direction;
    else return;
    e.preventDefault();
    onChange(clamp(value + delta, min, max));
  }

  return (
    <div
      className={`sash sash--${orientation}`}
      role="separator"
      aria-orientation={orientation}
      aria-label={label}
      aria-valuenow={Math.round(value)}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onPointerDown={(e) => {
        e.preventDefault();
        e.currentTarget.setPointerCapture?.(e.pointerId);
        drag.current = { start: axis(e), value };
        document.body.classList.add(`is-resizing-${orientation}`);
      }}
      onPointerMove={(e) => {
        if (!drag.current) return;
        onChange(clamp(drag.current.value + (axis(e) - drag.current.start) * direction, min, max));
      }}
      onPointerUp={(e) => {
        drag.current = null;
        e.currentTarget.releasePointerCapture?.(e.pointerId);
        document.body.classList.remove(`is-resizing-${orientation}`);
      }}
      onPointerCancel={() => {
        drag.current = null;
        document.body.classList.remove(`is-resizing-${orientation}`);
      }}
    />
  );
}
