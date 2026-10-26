import { useEffect, useRef, useState, type ReactNode } from "react";

import { useFocusTrap } from "../focus-trap";

/**
 * A modal shell: backdrop, focus trap, Escape to close, and focus returned to
 * whatever had it before. `initialFocus` names the element to land on.
 */
export function Modal({
  label,
  labelledBy,
  role = "dialog",
  className = "",
  onClose,
  initialFocus,
  children,
}: {
  label?: string;
  labelledBy?: string;
  role?: "dialog" | "alertdialog";
  className?: string;
  onClose: () => void;
  initialFocus?: React.RefObject<HTMLElement | null>;
  children: ReactNode;
}) {
  const dialog = useRef<HTMLDivElement>(null);
  useFocusTrap(dialog, true);

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    (initialFocus?.current ?? dialog.current)?.focus();
    return () => {
      if (previous?.isConnected) previous.focus();
    };
  }, [initialFocus]);

  return (
    <div className="backdrop" role="presentation" onMouseDown={onClose}>
      <div
        ref={dialog}
        className={`modal ${className}`}
        role={role}
        aria-modal="true"
        aria-label={label}
        aria-labelledby={labelledBy}
        tabIndex={-1}
        onMouseDown={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            onClose();
          }
        }}
      >
        {children}
      </div>
    </div>
  );
}

/** Ask before an irreversible action. Focus starts on Cancel. */
export function ConfirmDialog({
  title,
  body,
  confirmLabel,
  onConfirm,
  onClose,
}: {
  title: string;
  body: string;
  confirmLabel: string;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const cancel = useRef<HTMLButtonElement>(null);
  return (
    <Modal role="alertdialog" labelledBy="confirm-title" className="modal--small" onClose={onClose} initialFocus={cancel}>
      <h2 id="confirm-title" className="modal__title">
        {title}
      </h2>
      <p className="modal__body">{body}</p>
      <div className="modal__actions">
        <button ref={cancel} type="button" className="btn" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="btn btn--danger-solid"
          onClick={() => {
            onClose();
            onConfirm();
          }}
        >
          {confirmLabel}
        </button>
      </div>
    </Modal>
  );
}

/** Ask for a short name, for creating or renaming a folder. */
export function NameDialog({
  title,
  label,
  initial,
  confirmLabel,
  onSubmit,
  onClose,
}: {
  title: string;
  label: string;
  initial: string;
  confirmLabel: string;
  onSubmit: (name: string) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState(initial);
  const field = useRef<HTMLInputElement>(null);
  const trimmed = name.trim();
  return (
    <Modal labelledBy="name-title" className="modal--small" onClose={onClose} initialFocus={field}>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (trimmed === "") return;
          onClose();
          onSubmit(trimmed);
        }}
      >
        <h2 id="name-title" className="modal__title">
          {title}
        </h2>
        <label className="field">
          <span className="field__label">{label}</span>
          <input
            ref={field}
            className="input"
            type="text"
            maxLength={100}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <div className="modal__actions">
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn btn--primary" disabled={trimmed === ""}>
            {confirmLabel}
          </button>
        </div>
      </form>
    </Modal>
  );
}
