import { useRef, useState, type RefObject } from "react";

import { FILTERS, tokenize } from "../hooks/query";

/**
 * The search line. Filters are tinted in place as they are typed, so the
 * grammar teaches itself: a transparent input sits over a mirror that renders
 * the same text token by token. The scope chip on the left says what an empty
 * query browses and what a typed query is narrowed to.
 */
export default function QueryLine({
  value,
  onChange,
  inputRef,
  scopeLabel,
  scopeOpen,
  scopeButtonRef,
  onScope,
  onLeaveDown,
}: {
  value: string;
  onChange: (value: string) => void;
  inputRef: RefObject<HTMLInputElement>;
  scopeLabel: string;
  scopeOpen: boolean;
  scopeButtonRef: RefObject<HTMLButtonElement>;
  onScope: () => void;
  onLeaveDown: () => void;
}) {
  const mirror = useRef<HTMLDivElement>(null);
  const [focused, setFocused] = useState(false);

  function syncScroll() {
    if (mirror.current && inputRef.current) mirror.current.scrollLeft = inputRef.current.scrollLeft;
  }

  function insert(text: string) {
    const next = value === "" || value.endsWith(" ") ? `${value}${text}` : `${value} ${text}`;
    onChange(next);
    requestAnimationFrame(() => inputRef.current?.focus());
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown" || event.key === "Enter") {
      event.preventDefault();
      onLeaveDown();
    } else if (event.key === "Escape") {
      event.preventDefault();
      if (value !== "") onChange("");
      else onLeaveDown();
    }
  }

  return (
    <div className="query" role="search">
      <button
        ref={scopeButtonRef}
        type="button"
        className="query__scope"
        aria-haspopup="dialog"
        aria-expanded={scopeOpen}
        onClick={onScope}
        title="Choose what to browse"
      >
        <span className="query__scope-label">{scopeLabel}</span>
        <svg viewBox="0 0 10 6" aria-hidden="true" className="query__chevron">
          <path d="M1 1l4 4 4-4" fill="none" stroke="currentColor" strokeWidth="1.5" />
        </svg>
      </button>
      <div className="query__field">
        <div ref={mirror} className="query__mirror" aria-hidden="true">
          {tokenize(value).map((token, index) => (
            <span key={index} className={`tok tok--${token.kind}`}>
              {token.text}
            </span>
          ))}
        </div>
        <input
          ref={inputRef}
          className="query__input"
          type="text"
          spellCheck={false}
          autoComplete="off"
          autoCapitalize="off"
          enterKeyHint="search"
          aria-label="Search sessions"
          aria-describedby="query-help"
          placeholder="Search sessions, or type a filter like agent:codex"
          value={value}
          onChange={(event) => {
            onChange(event.target.value);
            syncScroll();
          }}
          onKeyDown={onKeyDown}
          onKeyUp={syncScroll}
          onSelect={syncScroll}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
        />
        <kbd className="query__key" aria-hidden="true">
          /
        </kbd>
      </div>
      <div className={`filters${focused && value === "" ? " is-shown" : ""}`}>
        <p id="query-help" className="filters__lead">Narrow a search with filters. Words without a filter are what Lore searches for.</p>
        <ul className="filters__list">
          {FILTERS.map((filter) => (
            <li key={filter.key}>
              <button
                type="button"
                tabIndex={-1}
                className="filters__item"
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => insert(filter.key.endsWith(":") ? filter.key : `${filter.key} `)}
              >
                <code>{filter.example}</code>
                <span>{filter.help}</span>
              </button>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
