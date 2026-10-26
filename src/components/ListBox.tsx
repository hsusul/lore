import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";

import { clamp } from "../format";
import { useWindowing } from "../virtual";

export type OpenVia = "keyboard" | "pointer";

interface ListBoxProps<T> {
  id: string;
  label: string;
  items: T[];
  keyOf: (item: T) => string;
  isOpen: (item: T) => boolean;
  renderRow: (item: T) => ReactNode;
  onOpen: (item: T, via: OpenVia) => void;
  listRef: RefObject<HTMLUListElement>;
  /** Arrow up from the first row hands focus back (to the query line). */
  onExitTop?: () => void;
  /** Called when the user reaches the end, by scrolling or by keyboard. */
  onReachEnd?: () => void;
  footer?: ReactNode;
}

/**
 * A virtualized single-select listbox. Arrow keys or j/k move the active row,
 * Home and End jump, Enter opens. Rows must share one fixed height so the
 * window math in `virtual.ts` holds.
 */
export default function ListBox<T>({
  id,
  label,
  items,
  keyOf,
  isOpen,
  renderRow,
  onOpen,
  listRef,
  onExitTop,
  onReachEnd,
  footer,
}: ListBoxProps<T>) {
  const listKey = items.length > 0 ? keyOf(items[0]) : null;
  const [cursor, setCursor] = useState({ listKey, index: 0 });
  const last = items.length - 1;
  const active = cursor.listKey === listKey ? clamp(cursor.index, 0, Math.max(last, 0)) : 0;
  const scroller = useRef<HTMLDivElement>(null);
  const { startIndex, endIndex, padTop, padBottom, scrollToIndex } = useWindowing(
    scroller,
    items.length,
  );

  useEffect(() => {
    scrollToIndex(active);
  }, [active, scrollToIndex]);

  function move(index: number) {
    setCursor({ listKey, index: clamp(index, 0, Math.max(last, 0)) });
    if (index > last) onReachEnd?.();
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLUListElement>) {
    const page = 10;
    switch (event.key) {
      case "ArrowDown":
      case "j":
        move(active + 1);
        break;
      case "ArrowUp":
      case "k":
        if (active === 0 && onExitTop) onExitTop();
        else move(active - 1);
        break;
      case "PageDown":
        move(active + page);
        break;
      case "PageUp":
        move(active - page);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(last);
        onReachEnd?.();
        break;
      case "Enter": {
        const item = items[active];
        if (item) onOpen(item, "keyboard");
        break;
      }
      default:
        return;
    }
    event.preventDefault();
  }

  function onScroll() {
    const el = scroller.current;
    if (el && el.scrollTop + el.clientHeight >= el.scrollHeight - 160) onReachEnd?.();
  }

  const optionId = (index: number) => `${id}-${index}`;

  return (
    <div ref={scroller} className="listbox" onScroll={onScroll}>
      <ul
        ref={listRef}
        id={id}
        className="listbox__list"
        role="listbox"
        aria-label={label}
        tabIndex={0}
        aria-activedescendant={items.length > 0 ? optionId(active) : undefined}
        onKeyDown={onKeyDown}
      >
        {padTop > 0 && <li aria-hidden="true" style={{ height: padTop }} />}
        {items.slice(startIndex, endIndex).map((item, offset) => {
          const index = startIndex + offset;
          const key = keyOf(item);
          const open = isOpen(item);
          return (
            <li
              key={key}
              id={optionId(index)}
              data-vrow
              role="option"
              aria-selected={open}
              aria-setsize={items.length}
              aria-posinset={index + 1}
              className={`row${index === active ? " is-active" : ""}${open ? " is-open" : ""}`}
              onClick={() => {
                setCursor({ listKey, index });
                onOpen(item, "pointer");
              }}
            >
              {renderRow(item)}
            </li>
          );
        })}
        {padBottom > 0 && <li aria-hidden="true" style={{ height: padBottom }} />}
      </ul>
      {footer}
    </div>
  );
}
