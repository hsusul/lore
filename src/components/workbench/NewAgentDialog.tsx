import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";

import type { TaskDto } from "../../ipc";
import { BranchIcon, CloseIcon, FolderIcon } from "./icons";
import NewAgentForm from "./NewAgentForm";
import { baseName } from "./state";

const FOCUSABLE =
  'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

type Props = {
  workspace: string;
  /** The branch agents start from, shown for context. */
  baseBranch?: string;
  /** Text to start the prompt with (the palette's "New agent: …"). */
  draft?: string;
  onClose: () => void;
  onCreated: (task: TaskDto) => void;
  onOpenFolder: () => void;
};

/**
 * Start an agent from anywhere without leaving what is on screen, the way
 * Linear's New issue works: a modal composer that closes on launch, Escape, or
 * a click outside, and hands focus back to where it was.
 */
export default function NewAgentDialog({ workspace, baseBranch, draft, onClose, onCreated, onOpenFolder }: Props) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  // Read during the first render: the prompt's autofocus (a child effect) runs
  // before any effect here could see what had focus.
  const [opener] = useState(() => document.activeElement as HTMLElement | null);

  useEffect(
    () => () => {
      if (opener && document.contains(opener)) opener.focus();
    },
    [opener],
  );

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    // The agent picker's menu is portaled outside the dialog and handles its own keys.
    if (!dialogRef.current?.contains(event.target as Node)) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;
    // Keep Tab inside the dialog: it is modal.
    const items = Array.from(dialogRef.current.querySelectorAll<HTMLElement>(FOCUSABLE));
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={onKeyDown}
      >
        <header className="dialog__header">
          <h2 id={titleId} className="dialog__title">
            New agent
          </h2>
          <span className="dialog__context" title={workspace}>
            <FolderIcon />
            {baseName(workspace)}
            {baseBranch && (
              <>
                <BranchIcon />
                {baseBranch}
              </>
            )}
          </span>
          <button type="button" className="wb-icon-btn" aria-label="Close" title="Close (Esc)" onClick={onClose}>
            <CloseIcon />
          </button>
        </header>
        <NewAgentForm
          workspace={workspace}
          onCreated={onCreated}
          onOpenFolder={onOpenFolder}
          initialPrompt={draft}
          autoFocus
        />
      </div>
    </div>
  );
}
