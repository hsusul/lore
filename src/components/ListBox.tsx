import { clsx } from "clsx";
import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";

import { clamp } from "../format";
import { useWindowing } from "../virtual";
import { Button } from "./ui/button";

/** How a row was opened: Enter hands focus to the reader; arrows and clicks keep it here. */
export type OpenVia = "keyboard" | "pointer" | "arrow";

export interface MoreState {
  label: string;
  status: "idle" | "loading" | "failed";
}

interface ListBoxProps<T> {
  id: string;
  label: string;
  items: T[];
  keyOf: (item: T) => string;
  isOpen: (item: T) => boolean;
  renderRow: (item: T) => ReactNode;
  onOpen: (item: T, via: OpenVia) => void;
  listRef: RefObject<HTMLUListElement>;
  /** Fixed row height: two-line rows, or the three-line rows of search results. */
  height: "row" | "hit-row";
  /** Arrow up from the first row hands focus back (to the search field). */
  onExitTop?: () => void;
  /** Called when the user reaches the end, by scrolling or by keyboard. */
  onReachEnd?: () => void;
  /** The "Load older" button, when there is more to load. */
  more?: MoreState | null;
}

/** Whether the window is key. Selection turns gray when it is not, as in Mail. */
function useWindowActive(): boolean {
  const [active, setActive] = useState(true);
  useEffect(() => {
    const on = () => setActive(true);
    const off = () => setActive(false);
    window.addEventListener("focus", on);
    window.addEventListener("blur", off);
    return () => {
      window.removeEventListener("focus", on);
      window.removeEventListener("blur", off);
    };
  }, []);
  return active;
}

/**
 * A virtualized single-select listbox in Mail's manner (spec 9.2, 11): the selection
 * is the open session and is the only focus indicator. Arrow keys or j/k open the
 * next or previous row, Enter moves focus into the reader. Rows must share one fixed
 * height so the window math in `virtual.ts` holds.
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
  height,
  onExitTop,
  onReachEnd,
  more,
}: ListBoxProps<T>) {
  const selected = items.findIndex(isOpen);
  const last = items.length - 1;
  const [listFocused, setListFocused] = useState(false);
  const windowActive = useWindowActive();
  const scroller = useRef<HTMLDivElement>(null);
  // The first row of the next page, which takes the selection once it arrives.
  const openWhenLoaded = useRef<number | null>(null);
  const { startIndex, endIndex, padTop, padBottom, scrollToIndex } = useWindowing(scroller, items.length);

  useEffect(() => {
    if (selected >= 0) scrollToIndex(selected);
  }, [selected, scrollToIndex]);

  useEffect(() => {
    const index = openWhenLoaded.current;
    if (index === null || items.length <= index) return;
    openWhenLoaded.current = null;
    onOpen(items[index], "arrow");
    listRef.current?.focus();
  }, [items, listRef, onOpen]);

  // Runs after the effect above, so a finished load that brought no new row (or
  // failed) drops the request instead of letting a later scroll load steal focus.
  const moreStatus = more?.status;
  useEffect(() => {
    if (moreStatus !== "loading") openWhenLoaded.current = null;
  }, [moreStatus]);

  function loadMore() {
    openWhenLoaded.current = items.length;
    onReachEnd?.();
  }

  function select(index: number) {
    const item = items[clamp(index, 0, Math.max(last, 0))];
    if (item) onOpen(item, "arrow");
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLUListElement>) {
    const page = 10;
    switch (event.key) {
      case "ArrowDown":
      case "j":
        if (selected >= last && more) loadMore();
        else select(selected + 1);
        break;
      case "ArrowUp":
      case "k":
        if (selected <= 0 && onExitTop) onExitTop();
        else select(selected - 1);
        break;
      case "PageDown":
        select(selected + page);
        break;
      case "PageUp":
        select(Math.max(selected - page, 0));
        break;
      case "Home":
        select(0);
        break;
      case "End":
        select(last);
        onReachEnd?.();
        break;
      case "Enter": {
        const item = items[Math.max(selected, 0)];
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

  // The top gap sits outside the scroller so row offsets stay index * row height for `virtual.ts`.
  return (
    <div ref={scroller} className="listbox mt-1.5" onScroll={onScroll}>
      <ul
        ref={listRef}
        id={id}
        className={clsx("listbox__list", selected < 0 && "focus-visible:shadow-focus-inset")}
        role="listbox"
        aria-label={label}
        tabIndex={0}
        data-focused={listFocused && windowActive ? "" : undefined}
        aria-activedescendant={selected >= 0 ? optionId(selected) : undefined}
        onKeyDown={onKeyDown}
        onFocus={() => setListFocused(true)}
        onBlur={() => setListFocused(false)}
      >
        {padTop > 0 && <li aria-hidden="true" style={{ height: padTop }} />}
        {items.slice(startIndex, endIndex).map((item, offset) => {
          const index = startIndex + offset;
          return (
            <li
              key={keyOf(item)}
              id={optionId(index)}
              data-vrow
              role="option"
              aria-selected={index === selected}
              aria-setsize={items.length}
              aria-posinset={index + 1}
              className={clsx("row", height === "row" ? "h-row" : "h-hit-row")}
              onClick={() => onOpen(item, "pointer")}
            >
              {renderRow(item)}
            </li>
          );
        })}
        {padBottom > 0 && <li aria-hidden="true" style={{ height: padBottom }} />}
      </ul>
      {more && (
        <div className="flex justify-center p-2.5">
          <Button disabled={more.status === "loading"} onClick={loadMore}>
            {more.status === "loading" ? "Loading…" : more.status === "failed" ? "Could Not Load More. Try Again" : more.label}
          </Button>
        </div>
      )}
    </div>
  );
}
