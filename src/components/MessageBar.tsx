import type { Flash } from "../hooks/workspace";
import { Button } from "./ui/button";

/**
 * The latest notice or error, in a line under the toolbar until dismissed. It takes the
 * status bar's message slot. Reader actions confirm on their own controls instead (spec
 * 9.12); this line keeps the remaining notices and errors.
 */
export default function MessageBar({ flash, onDismiss }: { flash: Flash; onDismiss: () => void }) {
  if (!flash) return null;
  return (
    <div className="flex flex-none items-center gap-3 bg-bg-content py-1.5 pl-5 pr-2.5 shadow-hairline-b select-text">
      <p role={flash.tone === "error" ? "alert" : "status"} className="min-w-0 flex-1 truncate text-callout text-label">
        {flash.text}
      </p>
      <Button onClick={onDismiss}>Dismiss</Button>
    </div>
  );
}
