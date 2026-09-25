import { useCallback, useState, type ReactNode } from "react";

import type { TaskDto } from "../../ipc";
import ExplorerTree from "./ExplorerTree";
import { RefreshIcon } from "./icons";
import type { DirCache } from "./useDirCache";

/**
 * A sidebar view's header row. In the workbench the Agents / Files switch takes
 * the title's place (one header, not two); the title stays for screen readers.
 */
export function SidebarHeader({
  title,
  switcher,
  children,
}: {
  title: string;
  switcher?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="wb-sidebar__header">
      {switcher ?? null}
      <h2 className={switcher ? "visually-hidden" : "wb-sidebar__title"}>{title}</h2>
      <div className="wb-sidebar__tools">{children}</div>
    </div>
  );
}

type ExplorerPanelProps = {
  workspace: string | null;
  tasks: TaskDto[] | null;
  /** "repo" or a task id. */
  scope: string;
  onScopeChange: (scope: string) => void;
  root: string | null;
  cache: DirCache;
  onOpenFile: (root: string, relPath: string) => void;
  onOpenFolder: () => void;
  activeFile: { root: string; relPath: string } | null;
  /** The Agents / Files switch, shown in place of the title. */
  switcher?: ReactNode;
};

/** The Explorer sidebar view: a scope selector and the lazy file tree. */
export function ExplorerPanel({
  workspace,
  tasks,
  scope,
  onScopeChange,
  root,
  cache,
  onOpenFile,
  onOpenFolder,
  activeFile,
  switcher,
}: ExplorerPanelProps) {
  const [expandedByRoot, setExpandedByRoot] = useState<Record<string, Set<string>>>({});
  const expanded = (root && expandedByRoot[root]) || EMPTY;

  const toggle = useCallback(
    (relPath: string, open: boolean) => {
      if (!root) return;
      setExpandedByRoot((prev) => {
        const next = new Set(prev[root] ?? []);
        if (open) next.add(relPath);
        else next.delete(relPath);
        return { ...prev, [root]: next };
      });
    },
    [root],
  );

  function refresh() {
    if (!root) return;
    void cache.load(root, "", true);
    for (const rel of expanded) void cache.load(root, rel, true);
  }

  return (
    <section className="wb-view" aria-label="Explorer">
      <SidebarHeader title="Explorer" switcher={switcher}>
        <button
          type="button"
          className="wb-icon-btn"
          aria-label="Refresh explorer"
          title="Refresh explorer"
          disabled={!root}
          onClick={refresh}
        >
          <RefreshIcon />
        </button>
      </SidebarHeader>
      <div className="wb-scope">
        <select
          className="wb-select"
          aria-label="Explorer scope"
          value={scope}
          onChange={(e) => onScopeChange(e.target.value)}
        >
          <option value="repo">Repository</option>
          {(tasks ?? []).map((task) => (
            <option key={task.id} value={task.id}>
              Worktree: {task.title}
            </option>
          ))}
        </select>
      </div>
      <div className="wb-view__body">
        {root ? (
          <ExplorerTree
            root={root}
            cache={cache}
            expanded={expanded}
            onToggle={toggle}
            onOpenFile={(rel) => onOpenFile(root, rel)}
            activeRelPath={activeFile && activeFile.root === root ? activeFile.relPath : null}
          />
        ) : (
          <div className="wb-empty">
            <p>{workspace ? "This worktree is not available." : "You have not yet opened a folder."}</p>
            {!workspace && (
              <button type="button" className="wb-btn wb-btn--primary" onClick={onOpenFolder}>
                Open folder…
              </button>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

const EMPTY = new Set<string>();
