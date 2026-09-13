import { BranchIcon, WarningIcon } from "./icons";

type Props = {
  workspace: string | null;
  runningCount: number;
  attentionCount: number;
  selectedBranch: string | null;
  onShowAgents: () => void;
};

/** Thin bottom status bar. */
export default function StatusBar({ workspace, runningCount, attentionCount, selectedBranch, onShowAgents }: Props) {
  return (
    <footer className="statusbar">
      <span className="statusbar__item statusbar__path" title={workspace ?? undefined}>
        {workspace ?? "No folder open"}
      </span>
      <span className="statusbar__spacer" />
      {selectedBranch && (
        <span className="statusbar__item" title="Selected agent branch">
          <BranchIcon />
          <span className="mono">{selectedBranch}</span>
        </span>
      )}
      {attentionCount > 0 && (
        <button
          type="button"
          className="statusbar__item statusbar__btn statusbar__attention"
          onClick={onShowAgents}
        >
          <WarningIcon />
          {attentionCount} need{attentionCount === 1 ? "s" : ""} attention
        </button>
      )}
      <button type="button" className="statusbar__item statusbar__btn" onClick={onShowAgents}>
        {runningCount > 0 && <span className="pulse-dot" aria-hidden="true" />}
        {runningCount} {runningCount === 1 ? "agent" : "agents"} running
      </button>
    </footer>
  );
}
