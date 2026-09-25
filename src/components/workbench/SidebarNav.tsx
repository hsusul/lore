import { ChevronIcon, FolderIcon, OverviewIcon, PlusIcon } from "./icons";
import { baseName, shortcut } from "./state";

type Props = {
  workspace: string | null;
  onOpenFolder: () => void;
  onNewAgent: () => void;
  onShowOverview: () => void;
  /** The overview is on stage (no agent or file open). */
  overviewActive: boolean;
  /** Agents that need the user, shown beside Overview. */
  attentionCount: number;
};

/** Top of the sidebar: the repository switcher, New agent, and Overview. */
export default function SidebarNav({
  workspace,
  onOpenFolder,
  onNewAgent,
  onShowOverview,
  overviewActive,
  attentionCount,
}: Props) {
  return (
    <nav className="side-nav" aria-label="Workspace">
      <button
        type="button"
        className="side-nav__workspace"
        title={workspace ? `${workspace}\nOpen another folder…` : "Open a git repository"}
        onClick={onOpenFolder}
      >
        <span className="side-nav__workspace-icon">
          <FolderIcon />
        </span>
        <span className="side-nav__workspace-name">{workspace ? baseName(workspace) : "Open folder…"}</span>
        <span className="side-nav__chevron">
          <ChevronIcon size={12} />
        </span>
      </button>
      <button type="button" className="side-nav__item" disabled={!workspace} onClick={onNewAgent}>
        <PlusIcon />
        <span className="side-nav__label">New agent</span>
        <kbd>{shortcut("N")}</kbd>
      </button>
      <button
        type="button"
        className={`side-nav__item${overviewActive ? " side-nav__item--on" : ""}`}
        aria-current={overviewActive || undefined}
        onClick={onShowOverview}
      >
        <OverviewIcon />
        <span className="side-nav__label">Overview</span>
        {attentionCount > 0 && (
          <span className="side-nav__count" title={`${attentionCount} need${attentionCount === 1 ? "s" : ""} you`}>
            {attentionCount}
          </span>
        )}
      </button>
    </nav>
  );
}
