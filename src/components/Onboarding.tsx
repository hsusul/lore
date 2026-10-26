import type { DetectedAgent } from "../ipc";

const KNOWN = [
  { id: "claude-code", name: "Claude Code", path: "~/.claude/projects" },
  { id: "codex", name: "Codex", path: "~/.codex/sessions" },
] as const;

/**
 * First run, shown only for a loaded archive that is truly empty. It names the
 * folders Lore will read and offers only actions this build can do.
 */
export default function Onboarding({
  agents,
  scanning,
  busyAgent,
  onScan,
  onAddRoot,
  onOpenSettings,
}: {
  agents: DetectedAgent[];
  scanning: boolean;
  busyAgent: string | null;
  onScan: () => void;
  onAddRoot: (agent: DetectedAgent) => void;
  onOpenSettings: () => void;
}) {
  const sources = KNOWN.map((known) => {
    const agent = agents.find((a) => a.id === known.id);
    return {
      known,
      agent: agent ?? {
        id: known.id,
        display_name: known.name,
        installed: false,
        version: null,
        session_count: 0,
        roots: [known.path],
        custom_roots: [],
      },
      detected: agent?.installed ?? false,
    };
  });

  return (
    <section className="onboarding" aria-labelledby="onboarding-title">
      <h2 id="onboarding-title" className="onboarding__title">
        Every agent session you have run, in one searchable place.
      </h2>
      <p className="onboarding__lead">
        Lore reads the session logs Claude Code and Codex already keep on disk, links them to the git repositories
        they worked in, and keeps the archive on this machine. Your original logs are never changed.
      </p>
      <ul className="onboarding__sources">
        {sources.map(({ known, agent, detected }) => (
          <li key={known.id} className="onboarding__source">
            <div>
              <strong>{known.name}</strong>
              <code title={agent.roots.join("\n")}>{agent.roots[0] ?? known.path}</code>
            </div>
            <span className={`flag ${detected ? "flag--ok" : ""}`}>{detected ? "found" : "not found"}</span>
            <button
              type="button"
              className="btn btn--quiet"
              disabled={busyAgent === known.id}
              onClick={() => onAddRoot(agent)}
            >
              {busyAgent === known.id ? "Adding folder…" : "Choose another folder…"}
            </button>
          </li>
        ))}
      </ul>
      <div className="onboarding__actions">
        <button type="button" className="btn btn--primary" disabled={scanning} onClick={onScan}>
          {scanning ? "Scanning agent history…" : "Scan agent history"}
        </button>
        <button type="button" className="btn" onClick={onOpenSettings}>
          Review sources and privacy
        </button>
      </div>
    </section>
  );
}
