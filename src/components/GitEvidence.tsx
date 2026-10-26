import { formatRelative, formatTime } from "../format";
import type { GitObservationDto, RepositorySummary, SegmentDto } from "../ipc";

/**
 * Provenance stays visible on every row: who claimed the git fact decides how
 * far to trust it, so the classes are never merged into one rail.
 */
const PROVENANCE: Record<string, { label: string; help: string }> = {
  agent_recorded: { label: "Agent recorded", help: "The agent wrote this down during the session." },
  agent_patch: { label: "Agent patch", help: "Taken from a patch the agent recorded." },
  lore_captured: { label: "Lore observed", help: "Lore read this from the repository." },
  lore_reverified: { label: "Lore re-verified", help: "Lore checked this again later." },
};

const TIMING: Record<string, string> = {
  exact_event: "at the event",
  near_event: "near the event",
  retrospective: "after the fact",
  current_only: "current state only",
};

export default function GitEvidence({
  observations,
  segments,
  repositories,
}: {
  observations: GitObservationDto[];
  segments: SegmentDto[];
  repositories: RepositorySummary[];
}) {
  const repoName = (id: string | null) =>
    id === null ? null : (repositories.find((r) => r.id === id)?.display_name ?? id);

  return (
    <div className="git">
      {observations.length === 0 ? (
        <p className="quiet">No git evidence was recorded or observed for this session.</p>
      ) : (
        <ol className="git__list">
          {observations.map((o) => {
            const provenance = PROVENANCE[o.source] ?? { label: o.source, help: "" };
            return (
              <li
                key={[o.segment_id, o.source, o.event_ts, o.observed_at, o.commit_sha, o.branch].join(":")}
                className={`obs obs--${o.source}`}
              >
                <span className="obs__source" title={provenance.help}>
                  {provenance.label}
                </span>
                <span className="obs__facts">
                  {o.branch && <code className="obs__branch">{o.branch}</code>}
                  {o.commit_sha && <code>{o.commit_sha.slice(0, 10)}</code>}
                  {o.is_dirty === true && <span className="flag flag--warn">uncommitted changes</span>}
                  {o.commit_exists === false && <span className="flag flag--danger">commit no longer exists</span>}
                  {o.remote_url_norm && <span className="quiet">{o.remote_url_norm}</span>}
                </span>
                <span className="obs__when" title={formatTime(o.event_ts ?? o.observed_at)}>
                  {TIMING[o.temporal_confidence] ?? o.temporal_confidence}, {formatRelative(o.observed_at)}
                </span>
              </li>
            );
          })}
        </ol>
      )}
      {segments.length > 0 && (
        <section aria-labelledby="segments-heading" className="segments">
          <h3 id="segments-heading" className="subhead">
            Working context
          </h3>
          <table className="segments__table">
            <thead>
              <tr>
                <th scope="col">Messages</th>
                <th scope="col">Directory</th>
                <th scope="col">Repository</th>
                <th scope="col">Model</th>
              </tr>
            </thead>
            <tbody>
              {segments.map((s) => (
                <tr key={s.id}>
                  <td>
                    {s.seq_start}–{s.seq_end}
                  </td>
                  <td>
                    <code>{s.cwd ?? "unknown"}</code>
                  </td>
                  <td>
                    {repoName(s.repository_id) ?? "unresolved"}
                    {s.repository_id && s.resolution_confidence !== "high" && (
                      <span className="quiet"> ({s.resolution_confidence} confidence)</span>
                    )}
                  </td>
                  <td>{s.model ?? "unknown"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
    </div>
  );
}
