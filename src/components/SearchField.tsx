import { clsx } from "clsx";
import { Search } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";

import { FILTERS, tokenize } from "../hooks/query";
import { ICON } from "./ui/button";
import { Popover, PopoverAnchor, PopoverContent } from "./ui/popover";

/**
 * The toolbar search field (spec 8): a fixed 260px capsule that never grows on focus.
 * Recognized filters are tinted in place: a transparent input sits over a mirror that
 * renders the same text token by token. The filter reference opens from the magnifier,
 * from `?` in an empty field, or with ⌘/, and never on plain focus.
 */
export default function SearchField({
  value,
  onChange,
  inputRef,
  onLeaveDown,
}: {
  value: string;
  onChange: (value: string) => void;
  inputRef: RefObject<HTMLInputElement>;
  onLeaveDown: () => void;
}) {
  const mirror = useRef<HTMLDivElement>(null);
  const [reference, setReference] = useState(false);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key === "/") {
        event.preventDefault();
        setReference((open) => !open);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  function syncScroll() {
    if (mirror.current && inputRef.current) mirror.current.scrollLeft = inputRef.current.scrollLeft;
  }

  function insert(text: string) {
    onChange(value === "" || value.endsWith(" ") ? `${value}${text}` : `${value} ${text}`);
    setReference(false);
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown" || event.key === "Enter") {
      event.preventDefault();
      onLeaveDown();
    } else if (event.key === "Escape") {
      event.preventDefault();
      if (value !== "") onChange("");
      else onLeaveDown();
    } else if (event.key === "?" && value === "") {
      event.preventDefault();
      setReference(true);
    }
  }

  return (
    <Popover open={reference} onOpenChange={setReference}>
      <PopoverAnchor asChild>
        <div
          role="search"
          className="relative ml-1.5 flex h-search-h w-search flex-none items-center gap-1 rounded-capsule bg-fill-2 pl-2 pr-2.5 text-control text-label-2 border-contrast border-separator focus-within:shadow-focus"
        >
          <button
            type="button"
            aria-label="Search Filters"
            aria-keyshortcuts="Meta+/"
            aria-expanded={reference}
            className="flex flex-none items-center rounded-capsule outline-none focus-visible:shadow-focus"
            onClick={() => setReference((open) => !open)}
          >
            <Search className={ICON.body} aria-hidden="true" />
          </button>
          <div className="relative h-full min-w-0 flex-1">
            <div
              ref={mirror}
              aria-hidden="true"
              className="pointer-events-none absolute inset-0 flex items-center overflow-hidden whitespace-pre text-control text-label"
            >
              {tokenize(value).map((token, index) => (
                <span key={index} className={clsx(token.kind === "filter" && "rounded-control bg-fill")}>
                  {token.text}
                </span>
              ))}
            </div>
            <input
              ref={inputRef}
              className="absolute inset-0 w-full bg-transparent text-control text-transparent caret-label outline-none placeholder:text-label-3"
              type="text"
              spellCheck={false}
              autoComplete="off"
              autoCapitalize="off"
              enterKeyHint="search"
              aria-label="Search sessions"
              placeholder="Search"
              value={value}
              onChange={(event) => {
                onChange(event.target.value);
                syncScroll();
              }}
              onKeyDown={onKeyDown}
              onKeyUp={syncScroll}
              onSelect={syncScroll}
            />
          </div>
        </div>
      </PopoverAnchor>
      <PopoverContent
        align="end"
        aria-label="Search filters"
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          inputRef.current?.focus();
        }}
      >
        <p className="text-callout text-label-2">Narrow a search with filters. Words without a filter are what Lore searches for.</p>
        <ul className="-mx-1.5 mt-2">
          {FILTERS.map((filter) => (
            <li key={filter.key}>
              <button
                type="button"
                className="flex h-menu-row w-full items-baseline gap-2 rounded-control px-1.5 text-left outline-none hover:bg-fill-2 focus-visible:shadow-focus"
                onClick={() => insert(filter.key.endsWith(":") ? filter.key : `${filter.key} `)}
              >
                <code className="flex-none font-mono text-callout text-label">{filter.example}</code>
                <span className="min-w-0 truncate text-subheadline text-label-2">{filter.help}</span>
              </button>
            </li>
          ))}
        </ul>
      </PopoverContent>
    </Popover>
  );
}
