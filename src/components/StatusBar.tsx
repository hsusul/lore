import { agentLabel } from "../format";
import type { DetectedAgent } from "../ipc";
import { scanFraction, type Flash, type ScanState } from "../hooks/workspace";

function ScanStatus({ scan }: { scan: ScanState }) {
  if (scan.phase === "idle") return null;
  if (scan.phase === "failed") return <span className="status__scan">Scan failed</span>;
  if (scan.phase === "done") {
    const p = scan.progress;
    return (
      <span className="status__scan">
        Scan finished: {p.ingested} new or changed, {p.skipped} unchanged
        {p.failed > 0 ? `, ${p.failed} unreadable` : ""}
      </span>
    );
  }
  const fraction = scanFraction(scan.progress);
  const percent = fraction ? Math.round((fraction.done / fraction.total) * 100) : 0;
  return (
    <span className="status__scan">
      <span
        className="meter"
        role="progressbar"
        aria-label="Archive scan"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={fraction ? percent : undefined}
        aria-valuetext={fraction ? `${fraction.done} of ${fraction.total} sessions` : "Looking for sessions"}
      >
        <span className="meter__fill" style={{ width: fraction ? `${percent}%` : "0%" }} />
      </span>
      {fraction ? `Scanning ${fraction.done} of ${fraction.total}` : "Looking for sessions…"}
    </span>
  );
}

/** Archive facts on the left, the latest message in the middle, the scan on the right. */
export default function StatusBar({
  agents,
  flash,
  scan,
  onDismiss,
}: {
  agents: DetectedAgent[];
  flash: Flash;
  scan: ScanState;
  onDismiss: () => void;
}) {
  const known = agents.filter((a) => a.session_count > 0 || a.installed);
  return (
    <footer className="status">
      <span className="status__agents">
        {known.map((agent) => (
          <span key={agent.id}>
            {agentLabel(agent.id)} <span className="count">{agent.session_count}</span>
          </span>
        ))}
      </span>
      <span className="status__flash">
        <span role="status" className="status__text">
          {flash?.tone === "notice" ? flash.text : ""}
        </span>
        <span role="alert" className="status__text status__text--error">
          {flash?.tone === "error" ? flash.text : ""}
        </span>
        {flash && (
          <button type="button" className="btn btn--quiet btn--tiny" onClick={onDismiss}>
            Dismiss
          </button>
        )}
      </span>
      <span role="status">
        <ScanStatus scan={scan} />
      </span>
    </footer>
  );
}
