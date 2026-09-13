import { useLayoutEffect, useRef } from "react";

import type { ActivityDto } from "../../ipc";

const KIND_LABELS: Record<ActivityDto["kind"], string> = {
  message: "Message",
  tool: "Tool",
  result: "Result",
  error: "Error",
  output: "Output",
};

type Props = {
  items: ActivityDto[] | null;
  error: string | null;
  compact?: boolean;
  label: string;
};

/**
 * An agent's activity timeline. Stays pinned to the bottom while the reader is
 * already at the bottom; scrolling up stops the auto-scroll.
 */
export default function ActivityList({ items, error, compact = false, label }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && stickRef.current) el.scrollTop = el.scrollHeight;
  }, [items]);

  return (
    <div
      ref={scrollRef}
      className={`activity${compact ? " activity--compact" : ""}`}
      onScroll={(e) => {
        const el = e.currentTarget;
        stickRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
      }}
    >
      {error && (
        <p className="wb-note wb-note--error" role="alert">
          {error}
        </p>
      )}
      {items === null ? (
        !error && (
          <p className="wb-note" role="status">
            Loading activity…
          </p>
        )
      ) : items.length === 0 ? (
        <p className="wb-note">No activity yet.</p>
      ) : (
        <ol className="activity__list" aria-label={label}>
          {items.map((item, i) => (
            <li key={i} className={`activity__item activity__item--${item.kind}`}>
              <span className="visually-hidden">{KIND_LABELS[item.kind]}: </span>
              {item.text}
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}
