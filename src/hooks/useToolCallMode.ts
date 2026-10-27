import { useCallback, useEffect, useRef, useState } from "react";

import { getJsonSetting, setSetting } from "../ipc";

/** "Show tool calls" (spec 9.4): one line per call, or every call opened to its input and output. */
export type ToolCallMode = "collapsed" | "expanded";

const KEY = "ui.toolCalls";

/**
 * The stored tool-call preference, read once at launch; a choice made before the read
 * returns wins. A failure to read or write it only costs the preference, so errors are
 * dropped.
 */
export function useToolCallMode(): [ToolCallMode, (mode: ToolCallMode) => void] {
  const [mode, setMode] = useState<ToolCallMode>("collapsed");
  const chosen = useRef(false);

  useEffect(() => {
    let live = true;
    getJsonSetting<string>(KEY).then(
      (stored) => {
        if (live && !chosen.current && (stored === "collapsed" || stored === "expanded")) setMode(stored);
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, []);

  const choose = useCallback((next: ToolCallMode) => {
    chosen.current = true;
    setMode(next);
    setSetting(KEY, JSON.stringify(next)).catch(() => {});
  }, []);

  return [mode, choose];
}
