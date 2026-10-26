import { useEffect, useState } from "react";

/** Follow `value` after it has been still for `ms`. A blank value lands at once. */
export function useDebounced(value: string, ms: number): string {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    if (value.trim() === "") {
      setSettled(value);
      return;
    }
    const timer = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(timer);
  }, [value, ms]);
  return settled;
}
