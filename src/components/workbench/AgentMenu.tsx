import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";

import type { TaskAgent, TaskEffort, TaskPermission } from "../../ipc";
import {
  EFFORTS,
  effortLabel,
  isModelId,
  modelLabel,
  modelsFor,
  pickerLabel,
  type ModelOption,
} from "./agentOptions";
import { ChevronIcon, TickIcon } from "./icons";
import { AGENT_LABELS } from "./state";

export const PERMISSION_LABELS: Record<TaskPermission, string> = {
  edits: "Edits",
  auto: "Auto",
};

export const PERMISSION_HINTS: Record<TaskPermission, string> = {
  edits: "Claude may edit files but not run shell commands.",
  auto: "Claude's safety classifier approves routine actions like running tests.",
};

const AGENTS: TaskAgent[] = ["claude_code", "codex"];
const PERMISSIONS: TaskPermission[] = ["edits", "auto"];
const MENU_WIDTH = 260;
const ITEMS = '[role="menuitem"],[role="menuitemcheckbox"],[role="menuitemradio"],[role="radio"]';

type Pane = "root" | "agent" | "permission" | "model" | "custom" | "effort";

const PANE_TITLES: Record<Exclude<Pane, "root">, string> = {
  agent: "Agent",
  permission: "Permission",
  model: "Model",
  custom: "Other model",
  effort: "Effort",
};

type Props = {
  agent: TaskAgent;
  onAgentChange: (agent: TaskAgent) => void;
  /** Accessible name for the trigger (`Agent` in the new-agent form, `Next agent` in the composer). */
  label: string;
  disabled?: boolean;
  /** `up` opens above the trigger (composer); `down` opens below (sidebar). */
  placement?: "up" | "down";
  permission?: TaskPermission;
  onPermissionChange?: (permission: TaskPermission) => void;
  autoHandoff?: boolean;
  onAutoHandoffChange?: (value: boolean) => void;
  model?: string | null;
  onModelChange?: (model: string | null) => void;
  effort?: TaskEffort | null;
  onEffortChange?: (effort: TaskEffort | null) => void;
};

/**
 * Quiet text trigger + compact popover, in the Cursor model-picker idiom.
 * The menu is portaled to `document.body` so a clipped composer card cannot hide it.
 *
 * Keyboard: focus moves into the menu on open; ↑/↓/Home/End move between rows;
 * → or Enter opens a sub-pane and ← or Escape goes back; Escape on the root
 * pane (or Tab) closes and returns focus to the trigger.
 */
export default function AgentMenu({
  agent,
  onAgentChange,
  label,
  disabled = false,
  placement = "down",
  permission,
  onPermissionChange,
  autoHandoff,
  onAutoHandoffChange,
  model = null,
  onModelChange,
  effort = null,
  onEffortChange,
}: Props) {
  const full = onPermissionChange != null && onAutoHandoffChange != null;
  // Permission modes are Claude's; Codex always runs in its own sandbox.
  const showPermission = full && agent === "claude_code";
  const [open, setOpen] = useState(false);
  const [pane, setPane] = useState<Pane>("root");
  const [coords, setCoords] = useState<{ top?: number; bottom?: number; left: number; maxHeight: number } | null>(
    null,
  );
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  /** The pane row to focus after going back, so ← lands where → started. */
  const returnTo = useRef<Pane | null>(null);
  const menuId = useId();

  useEffect(() => {
    if (!open) setPane("root");
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current) {
      setCoords(null);
      return;
    }
    const anchor = triggerRef.current;
    function place() {
      const rect = anchor.getBoundingClientRect();
      const left = Math.min(Math.max(8, rect.left), Math.max(8, window.innerWidth - MENU_WIDTH - 8));
      // A long pane (every model) scrolls inside the room between the trigger and the window edge.
      if (placement === "up") {
        setCoords({ left, bottom: window.innerHeight - rect.top + 6, maxHeight: rect.top - 14 });
      } else {
        setCoords({ left, top: rect.bottom + 6, maxHeight: window.innerHeight - rect.bottom - 14 });
      }
    }
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, placement, pane]);

  // Move focus into the menu when it opens and whenever the pane changes.
  const shown = open && coords !== null;
  useLayoutEffect(() => {
    if (!shown || !menuRef.current) return;
    const back = returnTo.current;
    returnTo.current = null;
    const target =
      menuRef.current.querySelector<HTMLElement>("input") ||
      (back && menuRef.current.querySelector<HTMLElement>(`[data-pane="${back}"]`)) ||
      menuRef.current.querySelector<HTMLElement>('[aria-checked="true"]:not([role="menuitemcheckbox"])') ||
      menuRef.current.querySelector<HTMLElement>(ITEMS);
    target?.focus();
  }, [shown, pane]);

  function close(refocus: boolean) {
    setOpen(false);
    if (refocus) triggerRef.current?.focus();
  }

  function back() {
    returnTo.current = pane;
    // Other model is opened from the model list, so it goes back there.
    setPane(pane === "custom" ? "model" : "root");
  }

  useEffect(() => {
    if (!open) return;
    function onDoc(event: MouseEvent) {
      const target = event.target as Node;
      if (rootRef.current?.contains(target) || menuRef.current?.contains(target)) return;
      setOpen(false);
    }
    function onKey(event: globalThis.KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      if (pane !== "root") {
        back();
        return;
      }
      close(true);
    }
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
    // `back` and `close` only read refs and setters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, pane]);

  function onMenuKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    // In a text field the arrows and Home/End edit text; Escape still goes back.
    if (event.target instanceof HTMLInputElement && event.key !== "Tab") return;
    const rows = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>(ITEMS) ?? []).filter(
      (row) => !row.disabled,
    );
    const index = rows.indexOf(document.activeElement as HTMLButtonElement);
    let next: HTMLElement | undefined;
    switch (event.key) {
      case "ArrowDown":
        next = rows[(index + 1) % rows.length];
        break;
      case "ArrowUp":
        next = rows[(index - 1 + rows.length) % rows.length];
        break;
      case "Home":
        next = rows[0];
        break;
      case "End":
        next = rows[rows.length - 1];
        break;
      case "ArrowRight": {
        const target = (document.activeElement as HTMLElement | null)?.dataset.pane as Pane | undefined;
        if (target) setPane(target);
        break;
      }
      case "ArrowLeft":
        if (pane !== "root") back();
        break;
      case "Tab":
        // Leave the menu the way focus came in: from the trigger.
        close(true);
        return;
      default:
        return;
    }
    event.preventDefault();
    next?.focus();
  }

  function pickAgent(next: TaskAgent) {
    onAgentChange(next);
    if (model && !modelsFor(next).some((item) => item.id === model)) {
      onModelChange?.(null);
    }
    back();
  }

  function pickPermission(next: TaskPermission) {
    onPermissionChange?.(next);
    back();
  }

  function pickModel(next: string | null) {
    onModelChange?.(next);
    returnTo.current = "model";
    setPane("root");
  }

  function pickEffort(next: TaskEffort | null) {
    onEffortChange?.(next);
    back();
  }

  const triggerText = pickerLabel(AGENT_LABELS[agent], agent, model, effort);
  /** A model typed in under Other model rather than picked from the list. */
  const customModel = model && !modelsFor(agent).some((item) => item.id === model) ? model : null;

  const menu = shown ? (
    <div
      ref={menuRef}
      id={menuId}
      className="model-menu"
      role="menu"
      aria-label={pane === "root" ? `${label} settings` : PANE_TITLES[pane]}
      style={{ top: coords.top, bottom: coords.bottom, left: coords.left, maxHeight: coords.maxHeight }}
      onKeyDown={onMenuKeyDown}
    >
      {pane === "root" && (
        <>
          {full && (
            <div className="model-menu__group">
              <button
                type="button"
                role="menuitemcheckbox"
                aria-checked={Boolean(autoHandoff)}
                aria-label="Auto-handoff on usage limit"
                className="model-menu__row"
                onClick={() => onAutoHandoffChange?.(!autoHandoff)}
              >
                <span>Auto-handoff</span>
                <span className={`model-switch${autoHandoff ? " model-switch--on" : ""}`} aria-hidden="true" />
              </button>
              {showPermission && (
                <PaneRow pane="permission" value={PERMISSION_LABELS[permission ?? "edits"]} onOpen={setPane} />
              )}
            </div>
          )}
          <div className="model-menu__group">
            <PaneRow pane="agent" value={AGENT_LABELS[agent]} onOpen={setPane} />
            <PaneRow pane="model" value={modelLabel(agent, model)} onOpen={setPane} />
            <PaneRow pane="effort" value={effortLabel(effort)} onOpen={setPane} />
          </div>
        </>
      )}
      {pane !== "root" && (
        <button
          type="button"
          role="menuitem"
          aria-label={`Back from ${PANE_TITLES[pane]}`}
          className="model-menu__row model-menu__back"
          onClick={back}
        >
          <span className="model-menu__back-icon">
            <ChevronIcon size={12} />
          </span>
          {PANE_TITLES[pane]}
        </button>
      )}
      {pane === "agent" &&
        AGENTS.map((id) => (
          <OptionRow key={id} role="menuitemradio" checked={agent === id} onPick={() => pickAgent(id)}>
            {AGENT_LABELS[id]}
          </OptionRow>
        ))}
      {pane === "permission" && (
        <div role="radiogroup" aria-label="Permission">
          {PERMISSIONS.map((id) => (
            <OptionRow
              key={id}
              role="radio"
              checked={permission === id}
              hint={PERMISSION_HINTS[id]}
              onPick={() => pickPermission(id)}
            >
              {PERMISSION_LABELS[id]}
            </OptionRow>
          ))}
        </div>
      )}
      {pane === "model" && (
        <>
          <div role="radiogroup" aria-label="Model">
            <OptionRow role="radio" checked={!model} onPick={() => pickModel(null)}>
              Default
            </OptionRow>
            {modelGroups(modelsFor(agent)).map(([group, items]) => (
              <div key={group ?? "models"} role="group" aria-label={group}>
                {group && (
                  <div className="model-menu__heading" aria-hidden="true">
                    {group}
                  </div>
                )}
                {items.map((item) => (
                  <OptionRow key={item.id} role="radio" checked={model === item.id} onPick={() => pickModel(item.id)}>
                    {item.label}
                  </OptionRow>
                ))}
              </div>
            ))}
          </div>
          <PaneRow pane="custom" value={customModel ?? ""} onOpen={setPane} />
        </>
      )}
      {pane === "custom" && (
        <CustomModel agent={agent} initial={customModel ?? ""} onApply={(id) => pickModel(id)} />
      )}
      {pane === "effort" && (
        <div role="radiogroup" aria-label="Effort">
          <OptionRow role="radio" checked={!effort} onPick={() => pickEffort(null)}>
            Default
          </OptionRow>
          {EFFORTS.map((item) => (
            <OptionRow
              key={item.id}
              role="radio"
              checked={effort === item.id}
              hint={item.hint}
              onPick={() => pickEffort(item.id)}
            >
              {item.label}
            </OptionRow>
          ))}
        </div>
      )}
    </div>
  ) : null;

  return (
    <div className={`model-picker model-picker--${placement}`} ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        className="model-picker__trigger"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        disabled={disabled}
        onClick={() => setOpen((value) => !value)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            setOpen(true);
          }
        }}
      >
        {triggerText}
        <ChevronIcon size={12} />
      </button>
      {menu ? createPortal(menu, document.body) : null}
    </div>
  );
}

/** A root-pane row that drills into a sub-pane (Enter, click, or →). */
function PaneRow({ pane, value, onOpen }: { pane: Exclude<Pane, "root">; value: string; onOpen: (pane: Pane) => void }) {
  return (
    <button type="button" role="menuitem" data-pane={pane} className="model-menu__row" onClick={() => onOpen(pane)}>
      <span>{PANE_TITLES[pane]}</span>
      <span className="model-menu__value">
        {value}
        <ChevronIcon size={12} />
      </span>
    </button>
  );
}

/** A choice in a sub-pane; the hint is visible text, not only a tooltip. */
function OptionRow({
  role,
  checked,
  hint,
  onPick,
  children,
}: {
  role: "radio" | "menuitemradio";
  checked: boolean;
  hint?: string;
  onPick: () => void;
  children: ReactNode;
}) {
  const labelId = useId();
  const hintId = useId();
  return (
    <button
      type="button"
      role={role}
      aria-checked={checked}
      aria-labelledby={labelId}
      aria-describedby={hint ? hintId : undefined}
      title={hint}
      className={`model-menu__row${hint ? " model-menu__row--hinted" : ""}`}
      onClick={onPick}
    >
      <span className="model-menu__option">
        <span id={labelId}>{children}</span>
        {hint && (
          <span id={hintId} className="model-menu__hint">
            {hint}
          </span>
        )}
      </span>
      {checked && (
        <span className="model-menu__check">
          <TickIcon />
        </span>
      )}
    </button>
  );
}

/** Consecutive models under the same family heading, in list order. */
function modelGroups(models: ModelOption[]): [string | undefined, ModelOption[]][] {
  const groups: [string | undefined, ModelOption[]][] = [];
  for (const item of models) {
    const last = groups[groups.length - 1];
    if (last && last[0] === item.group) last[1].push(item);
    else groups.push([item.group, [item]]);
  }
  return groups;
}

/** Other model: any id the agent's CLI accepts, for models newer than Lore's list. */
function CustomModel({ agent, initial, onApply }: { agent: TaskAgent; initial: string; onApply: (id: string) => void }) {
  const [value, setValue] = useState(initial);
  const [invalid, setInvalid] = useState(false);
  const hintId = useId();
  return (
    <div className="model-menu__custom">
      <input
        className="model-menu__input"
        type="text"
        aria-label="Model id"
        aria-describedby={hintId}
        aria-invalid={invalid || undefined}
        placeholder={agent === "codex" ? "gpt-5.6-sol" : "claude-opus-5-5"}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        value={value}
        onChange={(e) => {
          setValue(e.target.value);
          setInvalid(false);
        }}
        onKeyDown={(e) => {
          if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
          e.preventDefault();
          const id = value.trim();
          if (isModelId(id)) onApply(id);
          else setInvalid(true);
        }}
      />
      <p id={hintId} className={`model-menu__hint${invalid ? " model-menu__hint--error" : ""}`}>
        {invalid
          ? "Use letters, digits, dots, dashes, and underscores."
          : `Any model id ${AGENT_LABELS[agent]} accepts. Enter to use it.`}
      </p>
    </div>
  );
}
