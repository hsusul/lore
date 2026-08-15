import type { DetectedAgent } from "../ipc";

const SUPPORTED_SOURCES = [
  { id: "claude-code", name: "Claude Code", path: "~/.claude/projects" },
  { id: "codex", name: "Codex", path: "~/.codex/sessions" },
] as const;

function detectionLabel(agent: DetectedAgent | undefined) {
  if (!agent) return "ready to scan";
  return agent.installed ? "detected" : "not detected";
}

/**
 * First-run guidance for a settled, genuinely empty archive. It states the
 * actual V0 privacy boundary and only offers actions the current build owns.
 */
export default function ArchiveOnboarding({
  agents,
  scanning,
  onScan,
  onOpenSettings,
}: {
  agents: DetectedAgent[];
  scanning: boolean;
  onScan: () => void;
  onOpenSettings: () => void;
}) {
  return (
    <section className="onboarding" aria-labelledby="onboarding-heading">
      <div className="onboarding__content">
        <p className="onboarding__eyebrow">Empty archive</p>
        <h2 id="onboarding-heading">Your agent history, searchable.</h2>
        <p className="onboarding__intro">
          Lore reads the sessions Claude Code and Codex already wrote to disk, then
          organizes them by repository and Git evidence.
        </p>

        <div className="onboarding__privacy" role="note">
          <strong>Local and read-only.</strong> Archive content stays on this machine, and
          Lore never changes your original agent logs.
        </div>

        <section aria-labelledby="sources-heading">
          <h3 id="sources-heading">What Lore checks</h3>
          <ul className="onboarding__sources">
            {SUPPORTED_SOURCES.map((source) => {
              const agent = agents.find((candidate) => candidate.id === source.id);
              const status = detectionLabel(agent);
              return (
                <li key={source.id}>
                  <span className="onboarding__source-name">{source.name}</span>
                  <code>{source.path}</code>
                  <span
                    className={`onboarding__source-status${agent?.installed ? " is-detected" : ""}`}
                  >
                    {status}
                  </span>
                </li>
              );
            })}
          </ul>
        </section>

        <div className="onboarding__actions">
          <button className="btn--primary" onClick={onScan} disabled={scanning}>
            {scanning ? "Scanning agent history…" : "Scan agent history"}
          </button>
          <button className="btn--ghost" onClick={onOpenSettings}>
            Review privacy and agents
          </button>
        </div>
      </div>
    </section>
  );
}
