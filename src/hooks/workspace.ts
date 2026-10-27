import type { ScanProgress, SearchSort } from "../ipc";
import type { AlertSpec, NameSpec } from "../components/Sheets";
import { agentFilter, agentQuery } from "./query";

/** What the list pane browses when no query is typed. */
export type Scope =
  | { kind: "all" }
  | { kind: "repository"; id: string }
  | { kind: "folder"; id: string };

export const ALL: Scope = { kind: "all" };

/**
 * A row in the sidebar or the scope menu: a scope to browse, or an agent, which is a
 * saved `agent:` search over everything.
 */
export type Source = Scope | { kind: "agent"; id: string };

export function sameSource(a: Source, b: Source): boolean {
  if (a.kind === "all" || b.kind === "all") return a.kind === b.kind;
  return a.kind === b.kind && a.id === b.id;
}

/** What the sidebar shows as selected: the agent whose filter is the whole query, else the scope. */
export function currentSource(ws: Pick<Workspace, "scope" | "query">): Source {
  const agent = agentFilter(ws.query);
  return agent !== null && ws.scope.kind === "all" ? { kind: "agent", id: agent } : ws.scope;
}

/**
 * The archive scan. Scans start from Rescan, from adding a source folder, or on
 * their own at launch, so progress events can move `idle` to `running` too.
 */
export type ScanState =
  | { phase: "idle" }
  | { phase: "running"; progress: ScanProgress | null }
  | { phase: "done"; progress: ScanProgress }
  | { phase: "failed"; error: string };

/** At most one overlay is open; opening another replaces it. */
export type Overlay =
  | { kind: "none" }
  | { kind: "palette" }
  | { kind: "settings" }
  | { kind: "alert"; alert: AlertSpec }
  | { kind: "name"; sheet: NameSpec };

export interface Workspace {
  scope: Scope;
  query: string;
  sort: SearchSort;
  /** The session open in the reader. */
  openId: string | null;
  overlay: Overlay;
  scan: ScanState;
}

export const initialWorkspace: Workspace = {
  scope: ALL,
  query: "",
  sort: "relevance",
  openId: null,
  overlay: { kind: "none" },
  scan: { phase: "idle" },
};

export type Action =
  | { type: "source"; source: Source }
  | { type: "query"; query: string }
  | { type: "sort"; sort: SearchSort }
  | { type: "open"; id: string | null }
  | { type: "overlay"; overlay: Overlay }
  | { type: "scan-requested" }
  | { type: "scan-progress"; progress: ScanProgress }
  | { type: "scan-failed"; error: string }
  | { type: "forgot"; id: string }
  | { type: "forgot-everything" };

export function workspaceReducer(state: Workspace, action: Action): Workspace {
  switch (action.type) {
    case "source": {
      const { source } = action;
      if (source.kind === "agent") return { ...state, scope: ALL, query: agentQuery(source.id), overlay: { kind: "none" } };
      // Leaving an agent row drops its filter; a typed search keeps running in the new scope.
      const query = currentSource(state).kind === "agent" ? "" : state.query;
      return { ...state, scope: source, query, overlay: { kind: "none" } };
    }
    case "query":
      return { ...state, query: action.query };
    case "sort":
      return { ...state, sort: action.sort };
    case "open":
      return { ...state, openId: action.id };
    case "overlay":
      return { ...state, overlay: action.overlay };
    case "scan-requested":
      return { ...state, scan: { phase: "running", progress: null } };
    case "scan-progress":
      return {
        ...state,
        scan: action.progress.done
          ? { phase: "done", progress: action.progress }
          : { phase: "running", progress: action.progress },
      };
    case "scan-failed":
      return { ...state, scan: { phase: "failed", error: action.error } };
    case "forgot":
      return state.openId === action.id ? { ...state, openId: null } : state;
    // Settings, which asked, stays open to say what was removed.
    case "forgot-everything":
      return { ...initialWorkspace, scan: state.scan, overlay: state.overlay };
  }
}

/** Processed and total candidates for a progress bar, or null before discovery. */
export function scanFraction(progress: ScanProgress | null): { done: number; total: number } | null {
  if (!progress) return null;
  const done = progress.ingested + progress.skipped + progress.failed;
  const total = Math.max(progress.discovered, done);
  return total > 0 ? { done, total } : null;
}
