import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import type { TaskAgent, TaskPermission } from "../../ipc";
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
const MENU_WIDTH = 240;

type Pane = "root" | "agent" | "permission";

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
};

/**
 * Quiet text trigger + compact popover, in the Cursor model-picker idiom.
 * The menu is portaled to `document.body` so a clipped composer card cannot hide it.
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
}: Props) {
  const full = onPermissionChange != null && onAutoHandoffChange != null;
  const [open, setOpen] = useState(false);
  const [pane, setPane] = useState<Pane>(full ? "root" : "agent");
  const [coords, setCoords] = useState<{ top?: number; bottom?: number; left: number } | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();

  useEffect(() => {
    if (!open) {
      setPane(full ? "root" : "agent");
    }
  }, [open, full]);

  useLayoutEffect(() => {
    if (!open || !rootRef.current) {
      setCoords(null);
      return;
    }
    const trigger = rootRef.current.querySelector<HTMLButtonElement>(".model-picker__trigger");
    if (!trigger) return;
    const anchor = trigger;
    function place() {
      const rect = anchor.getBoundingClientRect();
      const left = Math.min(Math.max(8, rect.left), Math.max(8, window.innerWidth - MENU_WIDTH - 8));
      if (placement === "up") {
        setCoords({ left, bottom: window.innerHeight - rect.top + 6 });
      } else {
        setCoords({ left, top: rect.bottom + 6 });
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

  useEffect(() => {
    if (!open) return;
    function onDoc(event: MouseEvent) {
      const target = event.target as Node;
      if (rootRef.current?.contains(target) || menuRef.current?.contains(target)) return;
      setOpen(false);
    }
    function onKey(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      if (full && pane !== "root") {
        setPane("root");
        return;
      }
      setOpen(false);
    }
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, full, pane]);

  function pickAgent(next: TaskAgent) {
    onAgentChange(next);
    setOpen(false);
  }

  function pickPermission(next: TaskPermission) {
    onPermissionChange?.(next);
    setPane("root");
  }

  const menu =
    open && coords ? (
      <div
        ref={menuRef}
        id={menuId}
        className="model-menu"
        role="menu"
        style={{ top: coords.top, bottom: coords.bottom, left: coords.left }}
      >
        {pane === "root" && full && (
          <>
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
              <button
                type="button"
                role="menuitem"
                className="model-menu__row"
                onClick={() => setPane("permission")}
              >
                <span>Permission</span>
                <span className="model-menu__value">
                  {PERMISSION_LABELS[permission ?? "edits"]}
                  <ChevronIcon size={12} />
                </span>
              </button>
            </div>
            <div className="model-menu__group">
              <button
                type="button"
                role="menuitem"
                className="model-menu__row"
                onClick={() => setPane("agent")}
              >
                <span>Agent</span>
                <span className="model-menu__value">
                  {AGENT_LABELS[agent]}
                  <ChevronIcon size={12} />
                </span>
              </button>
            </div>
          </>
        )}
        {pane === "agent" &&
          AGENTS.map((id) => (
            <button
              key={id}
              type="button"
              role="menuitemradio"
              aria-checked={agent === id}
              className="model-menu__row"
              onClick={() => pickAgent(id)}
            >
              {AGENT_LABELS[id]}
              {agent === id && (
                <span className="model-menu__check">
                  <TickIcon />
                </span>
              )}
            </button>
          ))}
        {pane === "permission" && (
          <div role="radiogroup" aria-label="Permission">
            {PERMISSIONS.map((id) => (
              <button
                key={id}
                type="button"
                role="radio"
                aria-checked={permission === id}
                title={PERMISSION_HINTS[id]}
                className="model-menu__row"
                onClick={() => pickPermission(id)}
              >
                {PERMISSION_LABELS[id]}
                {permission === id && (
                  <span className="model-menu__check">
                    <TickIcon />
                  </span>
                )}
              </button>
            ))}
          </div>
        )}
      </div>
    ) : null;

  return (
    <div className={`model-picker model-picker--${placement}`} ref={rootRef}>
      <button
        type="button"
        className="model-picker__trigger"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        disabled={disabled}
        onClick={() => setOpen((value) => !value)}
      >
        {AGENT_LABELS[agent]}
        <ChevronIcon size={12} />
      </button>
      {menu ? createPortal(menu, document.body) : null}
    </div>
  );
}
