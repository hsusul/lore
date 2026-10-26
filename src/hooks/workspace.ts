import type { ScanProgress, SearchSort } from "../ipc";

/** What the list pane browses when no query is typed. */
export type Scope =
  | { kind: "all" }
  | { kind: "repository"; id: string }
  | { kind: "folder"; id: string };

export const ALL: Scope = { kind: "all" };

export function sameScope(a: Scope, b: Scope): boolean {
  if (a.kind === "all" || b.kind === "all") return a.kind === b.kind;
  return a.kind === b.kind && a.id === b.id;
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

/** One transient line in the status bar. Errors stay until dismissed or replaced. */
export type Flash = { tone: "notice" | "error"; text: string } | null;

/** At most one overlay is open; opening another replaces it. */
export type Overlay =
  | { kind: "none" }
  | { kind: "palette" }
  | { kind: "settings" }
  | { kind: "scope" }
  | {
      kind: "confirm";
      title: string;
      body: string;
      confirmLabel: string;
      onConfirm: () => void;
    }
  | {
      kind: "name";
      title: string;
      label: string;
      initial: string;
      confirmLabel: string;
      onSubmit: (name: string) => void;
    };

export interface Workspace {
  scope: Scope;
  query: string;
  sort: SearchSort;
  /** The session open in the reader. */
  openId: string | null;
  overlay: Overlay;
  flash: Flash;
  scan: ScanState;
}

export const initialWorkspace: Workspace = {
  scope: ALL,
  query: "",
  sort: "relevance",
  openId: null,
  overlay: { kind: "none" },
  flash: null,
  scan: { phase: "idle" },
};

export type Action =
  | { type: "scope"; scope: Scope }
  | { type: "query"; query: string }
  | { type: "sort"; sort: SearchSort }
  | { type: "open"; id: string | null }
  | { type: "overlay"; overlay: Overlay }
  | { type: "flash"; flash: Flash }
  | { type: "scan-requested" }
  | { type: "scan-progress"; progress: ScanProgress }
  | { type: "scan-failed"; error: string }
  | { type: "forgot-everything" };

export function workspaceReducer(state: Workspace, action: Action): Workspace {
  switch (action.type) {
    case "scope":
      return { ...state, scope: action.scope, overlay: { kind: "none" } };
    case "query":
      return { ...state, query: action.query };
    case "sort":
      return { ...state, sort: action.sort };
    case "open":
      return { ...state, openId: action.id };
    case "overlay":
      return { ...state, overlay: action.overlay };
    case "flash":
      return { ...state, flash: action.flash };
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
      return {
        ...state,
        scan: { phase: "failed", error: action.error },
        flash: { tone: "error", text: action.error },
      };
    case "forgot-everything":
      return { ...initialWorkspace, scan: state.scan };
  }
}

/** Processed and total candidates for a progress bar, or null before discovery. */
export function scanFraction(progress: ScanProgress | null): { done: number; total: number } | null {
  if (!progress) return null;
  const done = progress.ingested + progress.skipped + progress.failed;
  const total = Math.max(progress.discovered, done);
  return total > 0 ? { done, total } : null;
}
