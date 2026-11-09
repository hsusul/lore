import { useCallback, useEffect, useMemo, useState } from "react";

import {
  acceptProposal,
  accountName,
  findLessons,
  listNotes,
  listProposals,
  rejectProposal,
  retireNote,
  skillsReport,
  skillUsage,
  type LessonAgentDto,
  type SkillFindingDto,
  type SkillUsageDto,
  type NoteDto,
  type ProposalDto,
} from "../ipc";
import { areasOf, CENTRE, layout, type Dot, type Item } from "../map/layout";
import { receipts, why } from "../map/proposal";
import { Blocks } from "./Blocks";
import { FindLessons } from "./FindLessons";
import { Mark } from "./Mark";
import { MapCanvas } from "./MapCanvas";
import { SkillCard } from "./SkillCard";
import { SessionsList, SessionView } from "./MapSessions";

/** The prototype's rail icons: 24-unit strokes in the current color. */
const ICONS = {
  search: (
    <>
      <circle cx="11" cy="11" r="6" />
      <path d="M20 20l-4-4" />
    </>
  ),
  today: (
    <>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 3v2M12 19v2M3 12h2M19 12h2" />
    </>
  ),
  notes: (
    <>
      <path d="M6 3h9l3 3v15H6z" />
      <path d="M9 11h6M9 15h4" />
    </>
  ),
  sessions: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="M3 9h18" />
    </>
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.3 5.3l2.1 2.1M16.6 16.6l2.1 2.1M5.3 18.7l2.1-2.1M16.6 7.4l2.1-2.1" />
    </>
  ),
};

function RailButton({
  icon,
  label,
  selected,
  onClick,
  className,
}: {
  icon: keyof typeof ICONS;
  label: string;
  selected?: boolean;
  onClick: () => void;
  className?: string;
}) {
  return (
    <button
      type="button"
      className={`mist-ni${selected ? " sel" : ""}${className ? ` ${className}` : ""}`}
      aria-label={label}
      data-tip={label}
      onClick={onClick}
    >
      <svg viewBox="0 0 24 24" aria-hidden="true">
        {ICONS[icon]}
      </svg>
    </button>
  );
}

type Data =
  | { status: "loading" }
  | { status: "failed"; error: string }
  | {
      status: "ready";
      lessons: ProposalDto[];
      notes: NoteDto[];
      person: string;
    };

type Page = "today" | "notes" | "sessions" | "find";

function LessonDetail({
  lesson,
  scope,
  busy,
  onAccept,
  onSkip,
  onReject,
  onRejectDuplicate,
  onFindMore,
}: {
  lesson: ProposalDto;
  scope: string;
  busy: boolean;
  onAccept: (title?: string) => void;
  onSkip: () => void;
  onReject: () => void;
  onRejectDuplicate: (of: string) => void;
  onFindMore: () => void;
}) {
  const repeats = lesson.repeats;
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(lesson.title);
  useEffect(() => {
    setEditing(false);
    setTitle(lesson.title);
  }, [lesson]);
  const reason = why(lesson.body);
  return (
    <>
      <div className="mist-kind">{scope}</div>
      {editing ? (
        <textarea
          className="mist-h2 mist-ed"
          aria-label="Rule"
          value={title}
          rows={2}
          onChange={(e) => setTitle(e.target.value)}
        />
      ) : (
        <h2 className="mist-h2">{lesson.title}</h2>
      )}
      {repeats && (
        <p className="mist-mt">
          {repeats.kind === "note"
            ? "Already a note: "
            : "Same as a lesson you haven't reviewed: "}
          {repeats.title}
        </p>
      )}
      {reason && <p className="mist-why">{reason}</p>}
      <ul className="mist-rc">
        {receipts(lesson.body).map((r, k) => (
          <li key={k}>
            <span className="mist-dot" />
            <div>
              <p>“{r.said}”</p>
              {r.where && <div className="mist-mt">{r.where}</div>}
            </div>
          </li>
        ))}
      </ul>
      {/* An update without rules leaves the target's as they are. */}
      {lesson.enforce.length > 0 && (
        <Blocks
          rules={lesson.enforce}
          before={lesson.updates === null ? undefined : lesson.target_enforce}
          pending
        />
      )}
      <div className="mist-acts">
        {editing ? (
          <>
            <button
              type="button"
              className="mist-btn pri"
              disabled={busy || !title.trim()}
              onClick={() => onAccept(title.trim())}
            >
              Save
            </button>
            <button
              type="button"
              className="mist-btn"
              onClick={() => setEditing(false)}
            >
              Cancel
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className="mist-btn pri"
              disabled={busy}
              aria-busy={busy}
              onClick={() => onAccept()}
            >
              Accept
            </button>
            <button
              type="button"
              className="mist-btn"
              onClick={() => setEditing(true)}
            >
              Edit
            </button>
            <button type="button" className="mist-btn" onClick={onSkip}>
              Ask again if it recurs
            </button>
            <button
              type="button"
              className="mist-btn"
              disabled={busy}
              onClick={onReject}
            >
              Reject
            </button>
            {repeats && (
              <button
                type="button"
                className="mist-btn"
                disabled={busy}
                onClick={() => onRejectDuplicate(repeats.id)}
              >
                Reject as duplicate
              </button>
            )}
          </>
        )}
      </div>
      <button type="button" className="mist-archive" onClick={onFindMore}>
        Find more lessons
      </button>
    </>
  );
}

function NoteDetail({
  note,
  scope,
  busy,
  onRetire,
}: {
  note: NoteDto;
  scope: string;
  busy: boolean;
  onRetire: () => void;
}) {
  return (
    <>
      <div className="mist-kind">{scope}</div>
      <h2 className="mist-h2">{note.title}</h2>
      {note.excerpt && <p className="mist-why">{note.excerpt}</p>}
      <Blocks rules={note.enforce} dropped={note.dropped_rules} />
      <div className="mist-acts">
        <button
          type="button"
          className="mist-btn"
          disabled={busy}
          aria-busy={busy}
          onClick={onRetire}
        >
          Retire
        </button>
      </div>
    </>
  );
}

/**
 * Lore's home, as variant-q.html draws it: the map fills the window, a small
 * floating rail sits at the left, and one card on the right holds the next
 * lesson to review or the item opened on the map.
 */
export function MapHome({
  onArchive,
  onSettings,
}: {
  onArchive: () => void;
  onSettings: () => void;
}) {
  const [data, setData] = useState<Data>({ status: "loading" });
  const [page, setPage] = useState<Page>("today");
  const [open, setOpen] = useState<Item | null>(null);
  const [cardShown, setCardShown] = useState(true);
  const [skipped, setSkipped] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [session, setSession] = useState<string | null>(null);
  const [focusSearch, setFocusSearch] = useState(0);
  const [finding, setFinding] = useState<string | null>(null);
  const [homeKey, setHomeKey] = useState(0);

  const [skills, setSkills] = useState<{
    usage: SkillUsageDto[];
    findings: SkillFindingDto[];
  } | null>(null);

  // Skill usage reads every turn in the archive, so it arrives after the map.
  const loadSkills = useCallback(async () => {
    try {
      const [usage, report] = await Promise.all([skillUsage(), skillsReport()]);
      setSkills({ usage, findings: report.findings });
    } catch {
      setSkills(null);
    }
  }, []);

  useEffect(() => {
    void loadSkills();
  }, [loadSkills]);

  const load = useCallback(async () => {
    try {
      const [lessons, notes, name] = await Promise.all([
        listProposals(),
        listNotes(),
        accountName().catch(() => null),
      ]);
      setData({
        status: "ready",
        lessons,
        notes,
        person: name?.split(" ")[0] || "Everywhere",
      });
    } catch (e) {
      setData({ status: "failed", error: String(e) });
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const model = useMemo(
    () =>
      data.status === "ready"
        ? layout(areasOf(data.lessons, data.notes, data.person, skills?.usage))
        : null,
    [data, skills],
  );
  const person = data.status === "ready" ? data.person : "";
  const lessons = data.status === "ready" ? data.lessons : [];
  const notes = data.status === "ready" ? data.notes : [];
  const waiting = lessons.filter((l) => !skipped.has(l.id));
  // Today is the next lesson to review, unless something on the map is open.
  const shown: Item | null =
    open ??
    (page === "today" && waiting[0]
      ? { kind: "lesson", lesson: waiting[0] }
      : null);

  const scopeName = (item: { scope: string; repo_name: string | null }) =>
    item.scope === CENTRE ? person : (item.repo_name ?? "Unlinked repository");

  const go = (next: Page) => {
    if (cardShown && page === next && !open) {
      setCardShown(false);
      return;
    }
    setPage(next);
    setOpen(null);
    setSession(null);
    setCardShown(true);
    setNotice(null);
  };

  const search = () => {
    setPage("sessions");
    setOpen(null);
    setSession(null);
    setCardShown(true);
    setFocusSearch((n) => n + 1);
  };

  const select = (dot: Dot) => {
    setOpen(dot.item);
    setSession(null);
    setCardShown(true);
    setNotice(null);
  };

  const act = async (run: () => Promise<string>, done: string) => {
    setBusy(true);
    try {
      await run();
      setOpen(null);
      setNotice(done);
      await load();
    } catch (e) {
      setNotice(String(e));
    } finally {
      setBusy(false);
    }
  };

  // Lessons the agent files show up as they would after any review: reload.
  const find = async (agent: LessonAgentDto) => {
    setFinding(agent.label);
    setNotice(null);
    const before = lessons.length;
    try {
      await findLessons(agent.id);
      const filed = (await listProposals()).length - before;
      await load();
      setPage("today");
      setNotice(
        filed > 0
          ? `${agent.label} filed ${filed} ${filed === 1 ? "lesson" : "lessons"}.`
          : `${agent.label} found no new lessons.`,
      );
    } catch (e) {
      setNotice(String(e));
    } finally {
      setFinding(null);
    }
  };

  const shownId = shown
    ? shown.kind === "lesson"
      ? `lesson:${shown.lesson.id}`
      : shown.kind === "note"
        ? `note:${shown.note.id}`
        : `skill:${shown.skill.skill}`
    : null;

  return (
    <div className="mist">
      {data.status === "loading" && (
        <div className="mist-status" role="status">
          <Mark className="mist-mark mist-spin" />
          <p>Reading your lessons</p>
        </div>
      )}
      {data.status === "failed" && (
        <div className="mist-status" role="alert">
          <p>Lore could not read your lessons.</p>
          <p className="mist-t2">{data.error}</p>
          <button
            type="button"
            className="mist-btn pri"
            onClick={() => void load()}
          >
            Try again
          </button>
        </div>
      )}
      {model && (
        <MapCanvas
          model={model}
          selectedId={cardShown ? shownId : null}
          onDeselect={() => setOpen(null)}
          inset={cardShown ? 540 : 0}
          onSelect={select}
          homeKey={homeKey}
          label={`Map of ${model.dots.length} lessons and notes in ${model.clusters.length} areas`}
        />
      )}

      {waiting.length > 0 && (
        <button
          type="button"
          className="mist-waiting"
          onClick={() => go("today")}
        >
          <span className="mist-dot les" />
          {waiting.length} {waiting.length === 1 ? "lesson" : "lessons"} waiting
        </button>
      )}

      <nav className="mist-side" aria-label="Lore">
        <button
          type="button"
          className="mist-home"
          aria-label="Today"
          onClick={() => {
            setHomeKey((n) => n + 1);
            go("today");
          }}
        >
          <Mark />
        </button>
        <RailButton
          icon="search"
          label="Search"
          onClick={search}
          className="find"
        />
        <RailButton
          icon="today"
          label="Review"
          selected={cardShown && page === "today" && !open}
          onClick={() => go("today")}
        />
        <RailButton
          icon="notes"
          label="Notes"
          selected={cardShown && page === "notes" && !open}
          onClick={() => go("notes")}
        />
        <RailButton
          icon="sessions"
          label="Sessions"
          selected={cardShown && page === "sessions" && !open}
          onClick={() => go("sessions")}
        />
        <RailButton
          icon="settings"
          label="Settings"
          onClick={onSettings}
          className="set"
        />
      </nav>

      {cardShown && data.status === "ready" && (
        <section
          className={`mist-main${shown ? " mdet" : page === "sessions" ? " tall" : ""}`}
          aria-label={
            shown
              ? "Opened item"
              : page === "notes"
                ? "Notes"
                : page === "sessions"
                  ? "Sessions"
                  : "Today"
          }
        >
          <button
            type="button"
            className="mist-shut"
            aria-label="Close"
            onClick={() => {
              setCardShown(false);
              setOpen(null);
            }}
          >
            <svg
              viewBox="0 0 24 24"
              width="16"
              height="16"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.6"
              strokeLinecap="round"
              aria-hidden="true"
            >
              <path d="M6 6l12 12M18 6L6 18" />
            </svg>
          </button>
          <div className="mist-det">
            {notice && (
              <p className="mist-notice" role="status">
                {notice}
              </p>
            )}
            {shown?.kind === "lesson" && (
              <LessonDetail
                lesson={shown.lesson}
                scope={scopeName(shown.lesson)}
                busy={busy}
                onAccept={(title) =>
                  void act(
                    () => acceptProposal(shown.lesson.id, title),
                    "Accepted. Agents get it from their next session.",
                  )
                }
                onReject={() =>
                  void act(
                    () =>
                      rejectProposal(
                        shown.lesson.id,
                        "Rejected in the Lore app",
                      ).then(() => ""),
                    "Rejected.",
                  )
                }
                onRejectDuplicate={(of) =>
                  void act(
                    () =>
                      rejectProposal(shown.lesson.id, `duplicate of ${of}`).then(
                        () => "",
                      ),
                    "Rejected as a duplicate.",
                  )
                }
                onSkip={() => {
                  setSkipped((s) => new Set(s).add(shown.lesson.id));
                  setOpen(null);
                  setNotice(null);
                }}
                onFindMore={() => {
                  setPage("find");
                  setOpen(null);
                  setNotice(null);
                }}
              />
            )}
            {shown?.kind === "note" && (
              <NoteDetail
                note={shown.note}
                scope={scopeName(shown.note)}
                busy={busy}
                onRetire={() =>
                  void act(
                    () => retireNote(shown.note.id, "Retired in the Lore app"),
                    "Retired. Agents stop getting it.",
                  )
                }
              />
            )}
            {shown?.kind === "skill" && (
              <SkillCard
                skill={shown.skill}
                findings={skills?.findings ?? []}
                onApplied={(message) => {
                  setOpen(null);
                  setNotice(message);
                  void loadSkills();
                }}
              />
            )}
            {!shown && (page === "today" || page === "find") && (
              <FindLessons
                finding={finding}
                onFind={(agent) => void find(agent)}
              />
            )}
            {!shown &&
              page === "sessions" &&
              (session ? (
                <SessionView id={session} onBack={() => setSession(null)} />
              ) : (
                <>
                  <SessionsList onOpen={setSession} focusSearch={focusSearch} />
                  <button
                    type="button"
                    className="mist-archive"
                    onClick={onArchive}
                  >
                    Folders, exports and filters in the full archive
                  </button>
                </>
              ))}
            {!shown && page === "notes" && (
              <>
                <div className="mist-kind">Notes</div>
                <ul className="mist-list">
                  {notes.map((note) => (
                    <li key={note.id}>
                      <button
                        type="button"
                        className="mist-it"
                        onClick={() => setOpen({ kind: "note", note })}
                      >
                        <span className="mist-dot" />
                        <span className="mist-tx">
                          <span>{note.title}</span>
                          <span className="mist-mt">{scopeName(note)}</span>
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              </>
            )}
          </div>
        </section>
      )}
    </div>
  );
}
