import { clsx } from "clsx";
import { Check, CircleX, Search } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";

import type { SearchSort } from "../ipc";
import { FILTERS, joinQuery, pendingFilter, splitQuery } from "../hooks/query";
import { ICON } from "./ui/button";
import { Popover, PopoverAnchor, PopoverContent } from "./ui/popover";

type FilterValue = { value: string; count?: number };

/** Values the field can offer after a bare `key:`, keyed by that key. */
export type FilterValues = Partial<Record<string, FilterValue[]>>;

const SUGGESTION_TITLES: Record<string, string> = { "agent:": "Agents", "repo:": "Repositories", "has:": "Has" };
const HAS_VALUES: FilterValue[] = FILTERS.filter((f) => f.key.startsWith("has:")).map((f) => ({ value: f.key.slice("has:".length) }));

export const SORTS: { value: SearchSort; label: string }[] = [
  { value: "relevance", label: "Best Match" },
  { value: "newest", label: "Newest" },
  { value: "oldest", label: "Oldest" },
];

const ROW = "flex h-menu-row w-full items-center gap-1.5 rounded-control px-1.5 text-left outline-none";

/** A committed filter (spec 8): the key in mono secondary text, the value in the body face. */
function Token({ text, selected }: { text: string; selected: boolean }) {
  const split = text.indexOf(":") + 1;
  return (
    <span
      data-selected={selected ? "" : undefined}
      className="group flex h-token min-w-0 items-center rounded-control bg-fill px-1 text-callout data-selected:bg-selected"
    >
      <span className="flex-none font-mono text-token-key group-data-selected:text-on-selected">{text.slice(0, split)}</span>
      <span className="truncate text-label group-data-selected:text-on-selected">{text.slice(split)}</span>
    </span>
  );
}

/**
 * The toolbar search field (spec 8): a fixed 260px capsule that never grows on focus.
 * A filter followed by a space becomes a token in front of the text. Backspace at the
 * start of the text selects the last token and a second Backspace deletes it, as in
 * Mail. A bare `key:` offers values
 * in a suggestion popover. The filter reference opens from the magnifier, from `?` in
 * an empty field, or with ⌘/, and never on plain focus.
 */
export default function SearchField({
  value,
  onChange,
  inputRef,
  onLeaveDown,
  values,
  sort,
  onSort,
}: {
  value: string;
  onChange: (value: string) => void;
  inputRef: RefObject<HTMLInputElement>;
  onLeaveDown: () => void;
  values: FilterValues;
  sort: SearchSort;
  onSort: (sort: SearchSort) => void;
}) {
  const field = useRef<HTMLDivElement>(null);
  const [reference, setReference] = useState(false);
  const [focused, setFocused] = useState(false);
  const [dismissedFor, setDismissedFor] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const [tokenSelected, setTokenSelected] = useState(false);
  const { tokens, text } = splitQuery(value);

  const pending = pendingFilter(text);
  const options = pending
    ? ((pending.key === "has:" ? HAS_VALUES : values[pending.key]) ?? []).filter(
        (option) => !/\s/.test(option.value) && option.value.toLowerCase().startsWith(pending.partial.toLowerCase()),
      )
    : [];
  const suggesting = !reference && focused && pending !== null && options.length > 0 && dismissedFor !== text;
  // Focus enters the reference but never the suggestions, so only the reference hands it back.
  const lastPanel = useRef<"reference" | "suggestions">("reference");
  useEffect(() => {
    if (reference || suggesting) lastPanel.current = reference ? "reference" : "suggestions";
  }, [reference, suggesting]);
  // The last row opens the filter reference, as in the prototype's suggestion menu.
  const rowCount = options.length + 1;
  const current = Math.min(active, rowCount - 1);

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

  function setText(next: string) {
    setActive(0);
    setDismissedFor(null);
    onChange(joinQuery(tokens, next));
  }

  function insert(filter: string) {
    setText(text === "" || text.endsWith(" ") ? `${text}${filter}` : `${text} ${filter}`);
    setReference(false);
  }

  function choose(row: number) {
    if (!pending) return;
    if (row >= options.length) {
      setReference(true);
      return;
    }
    const typed = pending.key.length + pending.partial.length;
    const picked = options[row].value;
    setText(`${text.slice(0, text.length - typed)}${pending.key}${picked} `);
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    const input = event.currentTarget;
    const atStart = input.selectionStart === 0 && input.selectionEnd === 0;
    if (event.key !== "Backspace") setTokenSelected(false);
    if (suggesting && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
      event.preventDefault();
      setActive((current + (event.key === "ArrowDown" ? 1 : rowCount - 1)) % rowCount);
    } else if (suggesting && (event.key === "Enter" || event.key === "Tab")) {
      event.preventDefault();
      choose(current);
    } else if (suggesting && event.key === "Escape") {
      event.preventDefault();
      setDismissedFor(text);
    } else if (reference && event.key === "Escape") {
      event.preventDefault();
      setReference(false);
    } else if (event.key === "ArrowDown" || event.key === "Enter") {
      event.preventDefault();
      onLeaveDown();
    } else if (event.key === "Escape") {
      event.preventDefault();
      if (value !== "") onChange("");
      else onLeaveDown();
    } else if (event.key === "?" && value === "") {
      event.preventDefault();
      setReference(true);
    } else if (event.key === "Backspace" && tokens.length > 0 && atStart) {
      event.preventDefault();
      if (tokenSelected) onChange(joinQuery(tokens.slice(0, -1), text));
      setTokenSelected(!tokenSelected);
    }
  }

  const listId = "search-suggestions";
  const optionId = (row: number) => `${listId}-${row}`;

  return (
    <Popover
      open={reference || suggesting}
      onOpenChange={(open) => {
        if (open) return;
        setReference(false);
        setDismissedFor(text);
      }}
    >
      <PopoverAnchor asChild>
        <div
          ref={field}
          role="search"
          className="ml-1.5 flex h-search-h w-search flex-none items-center gap-1 rounded-capsule bg-fill-2 pl-2 pr-1.5 text-control text-label-2 border-contrast border-separator focus-within:shadow-focus"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) {
              event.preventDefault();
              inputRef.current?.focus();
            }
          }}
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
          <div className="flex h-full min-w-0 flex-1 items-center gap-1 overflow-hidden">
            {tokens.map((token, index) => (
              <Token key={`${index}:${token}`} text={token} selected={tokenSelected && index === tokens.length - 1} />
            ))}
            <input
              ref={inputRef}
              className="h-full min-w-8 flex-1 bg-transparent text-control text-label outline-none placeholder:text-label-3"
              type="text"
              role="combobox"
              aria-expanded={suggesting}
              aria-controls={suggesting ? listId : undefined}
              aria-autocomplete="list"
              aria-activedescendant={suggesting ? optionId(current) : undefined}
              spellCheck={false}
              autoComplete="off"
              autoCapitalize="off"
              enterKeyHint="search"
              aria-label="Search sessions"
              placeholder={tokens.length === 0 ? "Search" : undefined}
              value={text}
              onChange={(event) => setText(event.target.value)}
              onKeyDown={onKeyDown}
              onFocus={() => setFocused(true)}
              onBlur={() => {
                setFocused(false);
                setTokenSelected(false);
              }}
            />
          </div>
          {value !== "" && (
            <button
              type="button"
              aria-label="Clear Search"
              className="flex flex-none items-center rounded-capsule text-label-3 outline-none focus-visible:shadow-focus"
              onClick={() => {
                onChange("");
                inputRef.current?.focus();
              }}
            >
              <CircleX className={ICON.small} aria-hidden="true" />
            </button>
          )}
        </div>
      </PopoverAnchor>
      <PopoverContent
        align="end"
        aria-label={reference ? "Search filters" : "Filter suggestions"}
        onOpenAutoFocus={(event) => {
          if (!reference) event.preventDefault();
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          if (lastPanel.current === "reference") inputRef.current?.focus();
        }}
        onInteractOutside={(event) => {
          if (field.current?.contains(event.target as Node)) event.preventDefault();
        }}
        onEscapeKeyDown={(event) => {
          if (field.current?.contains(event.target as Node)) event.preventDefault();
        }}
      >
        {reference ? (
          <>
            <p className="text-callout text-label-2">Narrow a search with filters. Words without a filter are what Lore searches for.</p>
            <ul className="-mx-1.5 mt-2">
              {FILTERS.map((filter) => (
                <li key={filter.key}>
                  <button
                    type="button"
                    className={clsx(ROW, "items-baseline hover:bg-fill-2 focus-visible:shadow-focus")}
                    onClick={() => insert(filter.key.endsWith(":") ? filter.key : `${filter.key} `)}
                  >
                    <code className="flex-none font-mono text-callout text-label">{filter.example}</code>
                    <span className="min-w-0 truncate text-subheadline text-label-2">{filter.help}</span>
                  </button>
                </li>
              ))}
            </ul>
            <p className="mt-2 text-subheadline font-semibold text-label-2">Sort By</p>
            <ul className="-mx-1.5 mt-1" role="radiogroup" aria-label="Sort by">
              {SORTS.map((option) => (
                <li key={option.value}>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={sort === option.value}
                    className={clsx(ROW, "hover:bg-fill-2 focus-visible:shadow-focus")}
                    onClick={() => onSort(option.value)}
                  >
                    <Check className={clsx(ICON.small, sort === option.value ? "text-label" : "text-transparent")} aria-hidden="true" />
                    {option.label}
                  </button>
                </li>
              ))}
            </ul>
          </>
        ) : (
          pending && (
            <div className="-m-1.5 select-none">
              <p className="px-1.5 pb-1 text-subheadline font-semibold text-label-2">{SUGGESTION_TITLES[pending.key] ?? pending.key}</p>
              <ul id={listId} role="listbox" aria-label={`Values for ${pending.key}`}>
                {options.map((option, row) => (
                  <li
                    key={option.value}
                    id={optionId(row)}
                    role="option"
                    aria-selected={row === current}
                    className={clsx(ROW, "group cursor-default", row === current && "bg-selected text-on-selected")}
                    onMouseDown={(event) => event.preventDefault()}
                    onMouseMove={() => setActive(row)}
                    onClick={() => choose(row)}
                  >
                    <span className="font-mono text-callout text-token-key group-aria-selected:text-on-selected">{pending.key}</span>
                    <span className="min-w-0 flex-1 truncate">{option.value}</span>
                    {option.count !== undefined && (
                      <span className="text-subheadline tabular-nums text-label-2 group-aria-selected:text-on-selected">{option.count}</span>
                    )}
                  </li>
                ))}
                <li role="none" className="mx-1.5 my-1.5 h-px bg-separator" />
                <li
                  id={optionId(options.length)}
                  role="option"
                  aria-selected={current === options.length}
                  className={clsx(ROW, "group cursor-default", current === options.length && "bg-selected text-on-selected")}
                  onMouseDown={(event) => event.preventDefault()}
                  onMouseMove={() => setActive(options.length)}
                  onClick={() => choose(options.length)}
                >
                  <span className="flex-1">Filter Reference</span>
                  <span className="text-subheadline text-label-2 group-aria-selected:text-on-selected">⌘/</span>
                </li>
              </ul>
            </div>
          )
        )}
      </PopoverContent>
    </Popover>
  );
}
