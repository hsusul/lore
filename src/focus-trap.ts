import { useEffect, type RefObject } from "react";

/**
 * Trap Tab focus within `ref` while `active`. Wraps focus from the last
 * focusable element back to the first (and vice-versa on Shift+Tab), so a modal
 * dialog cannot leak focus to the inert background behind it — the WAI-ARIA
 * requirement that `aria-modal` alone does not enforce. Escape handling and
 * initial focus stay with the caller.
 */
export function useFocusTrap(ref: RefObject<HTMLElement | null>, active: boolean) {
  useEffect(() => {
    if (!active) return;
    const el = ref.current;
    if (!el) return;
    const container = el;

    function focusable(): HTMLElement[] {
      return Array.from(
        container.querySelectorAll<HTMLElement>(
          'a[href]:not([aria-hidden="true"]), ' +
            'button:not([disabled]):not([aria-hidden="true"]), ' +
            'input:not([disabled]):not([type="hidden"]):not([aria-hidden="true"]), ' +
            'textarea:not([disabled]):not([aria-hidden="true"]), ' +
            'select:not([disabled]):not([aria-hidden="true"]), ' +
            '[tabindex]:not([tabindex="-1"]):not([aria-hidden="true"])',
        ),
      );
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Tab") return;
      const nodes = focusable();
      if (nodes.length === 0) {
        event.preventDefault();
        return;
      }
      const first = nodes[0];
      const last = nodes[nodes.length - 1];
      const activeEl = document.activeElement;

      if (event.shiftKey && (activeEl === first || !container.contains(activeEl))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && activeEl === last) {
        event.preventDefault();
        first.focus();
      }
    }

    el.addEventListener("keydown", onKeyDown);
    return () => el.removeEventListener("keydown", onKeyDown);
  }, [ref, active]);
}
