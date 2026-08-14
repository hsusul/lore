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

    function focusable(): HTMLElement[] {
      return Array.from(
        el!.querySelectorAll<HTMLElement>(
          'a[href], button:not([disabled]), input:not([disabled]), ' +
            'textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      );
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Tab") return;
      const nodes = focusable();
      if (nodes.length === 0) return;
      const first = nodes[0];
      const last = nodes[nodes.length - 1];
      const activeEl = document.activeElement;

      if (event.shiftKey && (activeEl === first || !el!.contains(activeEl))) {
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
