import { useRef } from "react";

/**
 * Run a menu item's action once its menu has closed. An open Radix menu traps
 * focus and hands it back to its trigger as it closes, so a dialog opened
 * straight from `onSelect` loses focus to the trigger behind it. Wrap the item's
 * action in `afterClose` and pass `onCloseAutoFocus` to the menu content.
 */
export function useAfterMenuClose() {
  const pending = useRef<(() => void) | null>(null);
  return {
    afterClose: (action: () => void) => () => {
      pending.current = action;
    },
    onCloseAutoFocus: () => {
      const action = pending.current;
      pending.current = null;
      // A microtask runs after Radix refocuses the trigger, so the dialog returns focus there.
      if (action) queueMicrotask(action);
    },
  };
}
