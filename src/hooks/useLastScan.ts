import { useEffect, useState } from "react";

import { getJsonSetting, setSetting } from "../ipc";
import type { ScanState } from "./workspace";

const KEY = "ui.lastScanAt";

/**
 * When the archive last finished a scan, for the Rescan tooltip (spec 18.8). Read once at
 * launch and recorded whenever a scan finishes. A failure to read or write it only costs
 * the tooltip its time, so errors are dropped.
 */
export function useLastScan(scan: ScanState): number | null {
  const [at, setAt] = useState<number | null>(null);

  useEffect(() => {
    let live = true;
    getJsonSetting<number>(KEY).then(
      (stored) => {
        if (live && typeof stored === "number") setAt((current) => current ?? stored);
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, []);

  const finished = scan.phase === "done";
  useEffect(() => {
    if (!finished) return;
    const now = Date.now();
    setAt(now);
    setSetting(KEY, JSON.stringify(now)).catch(() => {});
  }, [finished]);

  return at;
}
