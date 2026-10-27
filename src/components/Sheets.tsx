import { useRef, useState, type FormEvent } from "react";

import appIcon from "../../src-tauri/icons/128x128.png";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./ui/dialog";
import { TextField } from "./ui/form";

/** A destructive confirmation: what it asks, what stays and what goes, and the action's name. */
export interface AlertSpec {
  title: string;
  body: string;
  action: string;
  /** Runs the action. The alert stays open, showing the error, if it rejects. */
  onConfirm: () => Promise<unknown>;
}

/** Ask for a short name, for creating or renaming a folder. */
export interface NameSpec {
  title: string;
  label: string;
  initial: string;
  action: string;
  /** Saves the name. The sheet stays open, showing the error, if it rejects. */
  onSubmit: (name: string) => Promise<unknown>;
}

/** Runs an action that may fail; a failure is kept to show inside the sheet that asked. */
function useAttempt(onDone: () => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function attempt(run: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await run();
      onDone();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }
  return { busy, error, attempt };
}

/**
 * A macOS alert (spec 9.10): 260px, everything centered, the app icon, a bold title, a
 * small body, and two equal buttons with the destructive one in red text. Focus starts
 * on Cancel, and a click outside does not dismiss it.
 */
export function Alert({ alert, onClose }: { alert: AlertSpec; onClose: () => void }) {
  const cancel = useRef<HTMLButtonElement>(null);
  const { busy, error, attempt } = useAttempt(onClose);
  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent
        role="alertdialog"
        className="w-alert p-5 text-center"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          cancel.current?.focus();
        }}
        onPointerDownOutside={(event) => event.preventDefault()}
      >
        <img src={appIcon} alt="" className="mx-auto size-app-icon" />
        <DialogTitle className="mt-2.5 text-headline font-bold">{alert.title}</DialogTitle>
        <DialogDescription className="mt-1 text-subheadline text-label-2">{alert.body}</DialogDescription>
        {error && (
          <p role="alert" className="mt-2 text-subheadline text-red-text select-text">
            {error}
          </p>
        )}
        <div className="mt-4 flex gap-2">
          <div className="flex-1">
            <Button ref={cancel} className="w-full" disabled={busy} onClick={onClose}>
              Cancel
            </Button>
          </div>
          <div className="flex-1">
            <Button className="w-full" destructive disabled={busy} onClick={() => void attempt(alert.onConfirm)}>
              {alert.action}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** A small sheet with one name field, Cancel, and the action as the default button. */
export function NameSheet({ sheet, onClose }: { sheet: NameSpec; onClose: () => void }) {
  const [name, setName] = useState(sheet.initial);
  const { busy, error, attempt } = useAttempt(onClose);
  const trimmed = name.trim();
  const ready = trimmed !== "" && !busy;

  function submit(event: FormEvent) {
    event.preventDefault();
    if (ready) void attempt(() => sheet.onSubmit(trimmed));
  }

  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent className="w-sheet-sm p-5" aria-describedby={undefined}>
        <form onSubmit={submit}>
          <DialogTitle>{sheet.title}</DialogTitle>
          <div className="mt-3 flex items-center gap-2">
            <label htmlFor="name-sheet-field" className="flex-none text-body">
              {sheet.label}:
            </label>
            <TextField
              id="name-sheet-field"
              className="flex-1"
              type="text"
              maxLength={100}
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </div>
          {error && (
            <p role="alert" className="mt-2 text-callout text-red-text select-text">
              {error}
            </p>
          )}
          <div className="mt-5 flex justify-end gap-2">
            <Button disabled={busy} onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" variant={ready ? "default" : "push"} disabled={!ready}>
              {sheet.action}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}
