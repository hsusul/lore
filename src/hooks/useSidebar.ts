import { useCallback, useState, useSyncExternalStore } from "react";

/** Below 1000px the sidebar starts hidden (spec 7.2). */
const NARROW = "(max-width: 999px)";

function subscribe(onChange: () => void): () => void {
  const query = window.matchMedia?.(NARROW);
  query?.addEventListener("change", onChange);
  return () => query?.removeEventListener("change", onChange);
}

const isNarrow = () => window.matchMedia?.(NARROW).matches ?? false;

/**
 * Whether the sidebar shows. It follows the window width until the person toggles it
 * (⌃⌘S or the toolbar button); after that their choice holds at any width.
 */
export function useSidebar(): { visible: boolean; toggle: () => void } {
  const narrow = useSyncExternalStore(subscribe, isNarrow);
  const [choice, setChoice] = useState<boolean | null>(null);
  const visible = choice ?? !narrow;
  const toggle = useCallback(() => setChoice(!visible), [visible]);
  return { visible, toggle };
}
