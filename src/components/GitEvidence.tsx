import { Bot, CircleCheck, FilePenLine, Folder, GitBranch, GitCommitHorizontal, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

import { agentLabel, formatClock, formatDayAndTime, formatTime, plural } from "../format";
import type { GitObservationDto, RepositorySummary, SegmentDto, SessionSummary } from "../ipc";
import { ICON } from "./ui/button";

/**
 * Who claimed a git fact decides how far to trust it, so each observation names its
 * source in words and keeps its own icon (spec 9.6).
 */
const ICONS: Record<string, LucideIcon> = {
  agent_recorded: Bot,
  agent_patch: FilePenLine,
  lore_captured: GitCommitHorizontal,
  lore_reverified: CircleCheck,
};

const DAY_MS = 86_400_000;

function gap(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "moments";
  if (minutes < 60) return plural(minutes, "minute");
  const hours = Math.round(minutes / 60);
  if (hours < 24) return hours === 1 ? "an hour" : plural(hours, "hour");
  const days = Math.round(ms / DAY_MS);
  return days === 1 ? "a day" : plural(days, "day");
}

/** When an observation was made, relative to the session ("2 minutes after the session ended"). */
function relative(t: number, started: number | null, ended: number | null): string {
  if (started !== null && Math.abs(t - started) < 60_000) return "when the session started";
  if (ended !== null && t > ended) return `${gap(t - ended)} after the session ended`;
  if (started !== null && t < started) return `${gap(started - t)} before the session started`;
  if (started !== null) return "during the session";
  return "";
}

const Code = ({ children }: { children: ReactNode }) => <code className="font-mono text-callout">{children}</code>;

interface Line {
  key: string;
  icon: LucideIcon;
  at: number;
  sentence: ReactNode;
}

/**
 * One sentence per observation, newest first. Branch, commit and remote are each named
 * once, by the earliest observation that carries them.
 */
function observationLines(observations: GitObservationDto[], summary: SessionSummary): Line[] {
  const agent = agentLabel(summary.agent_id);
  const seen = new Set<string>();
  const fresh = (kind: string, value: string | null) => {
    if (value === null || seen.has(`${kind}:${value}`)) return null;
    seen.add(`${kind}:${value}`);
    return value;
  };
  const oldestFirst = [...observations].sort((a, b) => (a.event_ts ?? a.observed_at) - (b.event_ts ?? b.observed_at));
  const lines = oldestFirst.map((o, index): Line => {
    const at = o.event_ts ?? o.observed_at;
    const branch = fresh("branch", o.branch);
    const commit = fresh("commit", o.commit_sha);
    const remote = fresh("remote", o.remote_url_norm);
    const has = branch !== null || commit !== null || remote !== null;
    const subject = (
      <>
        {branch && (
          <>
            Branch <Code>{branch}</Code>
          </>
        )}
        {commit && (
          <>
            {branch ? " at commit " : "Commit "}
            <Code>{commit.slice(0, 7)}</Code>
          </>
        )}
        {remote && (
          <>
            {branch || commit ? " on " : "Remote "}
            <Code>{remote}</Code>
          </>
        )}
      </>
    );
    const when = relative(at, summary.started_at, summary.ended_at);
    const tail = when ? ` ${when}` : "";

    let claim: ReactNode;
    switch (o.source) {
      case "agent_recorded":
        claim = has ? <>{subject} recorded by {agent}{tail}.</> : <>{agent} recorded the repository{tail}.</>;
        break;
      case "agent_patch":
        claim = has ? <>{subject} taken from a patch {agent} recorded{tail}.</> : <>Taken from a patch {agent} recorded{tail}.</>;
        break;
      case "lore_captured":
        claim = has ? <>{subject} captured by Lore{tail}.</> : <>Lore read the repository{tail}.</>;
        break;
      case "lore_reverified":
        claim = has ? <>{subject} re-verified by Lore{tail}.</> : <>Re-verified by Lore{tail}.</>;
        break;
      default:
        claim = has ? <>{subject} from {o.source}{tail}.</> : <>Observed by {o.source}{tail}.</>;
    }
    const state = [
      o.commit_exists === true && "The commit still exists.",
      o.commit_exists === false && "The commit no longer exists.",
      o.is_dirty === true && "The working tree had uncommitted changes.",
      o.is_dirty === false && "The working tree was clean.",
      o.temporal_confidence === "current_only" && "This is the repository’s current state, not the session’s.",
    ].filter(Boolean);
    return {
      key: `${index}:${o.source}:${at}`,
      icon: ICONS[o.source] ?? GitBranch,
      at,
      sentence: (
        <>
          {claim}
          {state.length > 0 && ` ${state.join(" ")}`}
        </>
      ),
    };
  });
  return lines.reverse();
}

function contextSentence(segment: SegmentDto, many: boolean, summary: SessionSummary, repositories: RepositorySummary[]): ReactNode {
  const repo = segment.repository_id === null ? null : (repositories.find((r) => r.id === segment.repository_id)?.display_name ?? segment.repository_id);
  const span = many ? `Messages ${segment.seq_start + 1} to ${segment.seq_end + 1} ran` : "The session ran";
  const where = segment.cwd ? <> in <Code>{segment.cwd}</Code></> : " in a folder Lore could not see";
  const link =
    repo === null || segment.resolution_confidence === "unresolved"
      ? ", not linked to a repository"
      : `, linked to ${repo}${segment.resolution_confidence === "high" ? "" : ` with ${segment.resolution_confidence} confidence`}`;
  const model = segment.model && segment.model !== summary.primary_model ? ` using ${segment.model}` : "";
  return (
    <>
      {span}
      {where}
      {link}
      {model}.
    </>
  );
}

function Row({ icon: Icon, sentence, time }: { icon: LucideIcon; sentence: ReactNode; time?: ReactNode }) {
  return (
    <li className="grid grid-cols-icon-row items-start gap-x-2">
      <Icon className={`${ICON.body} mt-0.5 text-label-2`} aria-hidden="true" />
      <p>{sentence}</p>
      {time ?? <span />}
    </li>
  );
}

/** The Git tab (spec 9.6): git observations as sentences with their source, then where the session ran. */
export default function GitEvidence({
  observations,
  segments,
  repositories,
  summary,
}: {
  observations: GitObservationDto[];
  segments: SegmentDto[];
  repositories: RepositorySummary[];
  summary: SessionSummary;
}) {
  const sameDay = (t: number) => summary.started_at !== null && new Date(t).toDateString() === new Date(summary.started_at).toDateString();
  return (
    <div className="flex max-w-measure flex-col gap-5">
      {observations.length === 0 ? (
        <p className="text-callout text-label-2">No git evidence was recorded or observed for this session.</p>
      ) : (
        <ul className="flex flex-col gap-3" aria-label="Git observations">
          {observationLines(observations, summary).map((line) => (
            <Row
              key={line.key}
              icon={line.icon}
              sentence={line.sentence}
              time={
                <time className="pt-0.5 text-callout tabular-nums text-label-2" dateTime={new Date(line.at).toISOString()} title={formatTime(line.at)}>
                  {sameDay(line.at) ? formatClock(line.at) : formatDayAndTime(line.at)}
                </time>
              }
            />
          ))}
        </ul>
      )}
      {segments.length > 0 && (
        <section aria-labelledby="context-heading" className="flex flex-col gap-3">
          <h3 id="context-heading" className="text-subheadline font-semibold text-label-2">
            Working Context
          </h3>
          <ul className="flex flex-col gap-3">
            {segments.map((segment) => (
              <Row key={segment.id} icon={Folder} sentence={contextSentence(segment, segments.length > 1, summary, repositories)} />
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
