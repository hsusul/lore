import { useEffect, useState } from "react";

import { agentLabel } from "../format";
import {
  applySkillEdit,
  draftSkillEdit,
  type SkillDraftDto,
  type SkillFindingDto,
  type SkillUsageDto,
} from "../ipc";
import { hunks, lineDiff } from "../map/diff";
import { useAgent } from "./FindLessons";
import { Mark } from "./Mark";

/** The newest corrections shown; the draft reads up to eight. */
const SHOWN = 5;

const day = (ms: number) =>
  new Date(ms).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });

function usage(skill: SkillUsageDto): string {
  if (skill.uses === 0) return "Never used.";
  const times = `${skill.uses} ${skill.uses === 1 ? "use" : "uses"} in ${skill.sessions} ${skill.sessions === 1 ? "session" : "sessions"}`;
  return skill.last_ms == null
    ? `${times}.`
    : `${times}, last ${day(skill.last_ms)}.`;
}

type Draft =
  | { status: "idle" }
  | { status: "drafting"; label: string }
  | { status: "ready"; draft: SkillDraftDto }
  | { status: "failed"; error: string };

/**
 * One skill on the map: how it has been used, what the person said right
 * after it ran, what the doctor found, and, when they had to re-prompt, an
 * edit their agent drafts for them to apply.
 */
export function SkillCard({
  skill,
  findings,
  onApplied,
}: {
  skill: SkillUsageDto;
  findings: SkillFindingDto[];
  onApplied: (message: string) => void;
}) {
  const { agents, agent } = useAgent();
  const [draft, setDraft] = useState<Draft>({ status: "idle" });
  const [applying, setApplying] = useState(false);

  useEffect(() => setDraft({ status: "idle" }), [skill.skill]);

  const propose = async () => {
    if (!agent) return;
    setDraft({ status: "drafting", label: agent.label });
    try {
      setDraft({
        status: "ready",
        draft: await draftSkillEdit(agent.id, skill.skill),
      });
    } catch (e) {
      setDraft({ status: "failed", error: String(e) });
    }
  };

  const apply = async (d: SkillDraftDto) => {
    setApplying(true);
    try {
      await applySkillEdit(skill.skill, d.before, d.after);
      setDraft({ status: "idle" });
      onApplied(
        `Updated ${skill.skill}. The old SKILL.md is kept in Lore's backups.`,
      );
    } catch (e) {
      setDraft({ status: "failed", error: String(e) });
    } finally {
      setApplying(false);
    }
  };

  if (draft.status === "drafting") {
    return (
      <div role="status">
        <Mark className="mist-mark mist-spin mist-finding" />
        <h2 className="mist-h2">
          Drafting an edit{" "}
          <span className="mist-hw">
            {draft.label} is reading {skill.skill} and what you said after it.
          </span>
        </h2>
      </div>
    );
  }

  if (draft.status === "ready") {
    const changes = hunks(lineDiff(draft.draft.before, draft.draft.after));
    return (
      <>
        <div className="mist-kind">Proposed edit to {skill.skill}</div>
        <p className="mist-why">{draft.draft.summary}</p>
        {changes.length === 0 ? (
          <p className="mist-mt">No change to the file.</p>
        ) : (
          <pre className="mist-diff" aria-label="Changes">
            {changes.map((line, k) =>
              line === null ? (
                <span key={k} className="gap">
                  {"…\n"}
                </span>
              ) : (
                <span
                  key={k}
                  className={
                    line.op === "+" ? "add" : line.op === "-" ? "del" : ""
                  }
                >
                  {`${line.op === "=" ? " " : line.op} ${line.text}\n`}
                </span>
              ),
            )}
          </pre>
        )}
        <div className="mist-acts">
          {changes.length > 0 && (
            <button
              type="button"
              className="mist-btn pri"
              disabled={applying}
              aria-busy={applying}
              onClick={() => void apply(draft.draft)}
            >
              Apply
            </button>
          )}
          <button
            type="button"
            className="mist-btn"
            onClick={() => setDraft({ status: "idle" })}
          >
            Discard
          </button>
        </div>
      </>
    );
  }

  const mine = findings.filter((f) => f.skill === skill.skill);
  return (
    <>
      <div className="mist-kind">Skill</div>
      <h2 className="mist-h2">{skill.skill}</h2>
      <p className="mist-why">{usage(skill)}</p>
      {draft.status === "failed" && (
        <p className="mist-notice" role="alert">
          {draft.error}
        </p>
      )}
      {mine.map((f, k) => (
        <div key={k} className="mist-finding-row">
          <div className="mist-mt">Problem</div>
          <p>{f.description}</p>
          <div className="mist-mt">The fix</div>
          <p>{f.fix}</p>
        </div>
      ))}
      {skill.reprompts.length > 0 && (
        <ul className="mist-rc">
          {skill.reprompts.slice(0, SHOWN).map((r, k) => (
            <li key={k}>
              <span className="mist-dot" />
              <div>
                <p>“{r.text}”</p>
                <div className="mist-mt">
                  {agentLabel(r.agent_id)}
                  {r.at_ms != null && `, ${day(r.at_ms)}`}
                  {r.repo && `, ${r.repo}`}
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}
      {skill.reprompts.length > 0 && (
        <div className="mist-acts">
          {!skill.editable_path ? (
            <p className="mist-mt">
              A plugin or another source manages this skill, so an edit here
              would be undone.
            </p>
          ) : agent ? (
            <button
              type="button"
              className="mist-btn pri"
              onClick={() => void propose()}
            >
              Propose an edit with {agent.label}
            </button>
          ) : (
            agents && (
              <p className="mist-mt">
                Sign in to Claude Code or Codex to propose an edit.
              </p>
            )
          )}
        </div>
      )}
    </>
  );
}
