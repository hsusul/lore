import { useCallback, useEffect, useState } from "react";

import { CONFIRM_MS } from "../components/ui/metrics";

/**
 * A short confirmation a control shows in its own tooltip ("Copied", spec 9.12). Showing
 * a new note restarts the clock; the note clears itself.
 */
export function useNote<T = string>(): [T | null, (note: T) => void] {
  const [note, setNote] = useState<{ value: T } | null>(null);
  useEffect(() => {
    if (note === null) return;
    const timer = setTimeout(() => setNote(null), CONFIRM_MS);
    return () => clearTimeout(timer);
  }, [note]);
  const show = useCallback((value: T) => setNote({ value }), []);
  return [note === null ? null : note.value, show];
}
