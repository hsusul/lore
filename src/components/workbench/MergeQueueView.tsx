import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { getRepoSettings, setRepoTestCommand, type MergeQueueItemDto, type TaskDto } from "../../ipc";
import {
  ArrowDownIcon,
  ArrowUpIcon,
  CheckIcon,
  FailedIcon,
  PendingIcon,
  ProgressIcon,
  SkippedIcon,
} from "./icons";
import { errorText } from "./state";
import type { MergeQueueEntry } from "./useMergeQueue";

/**
 * Whether the backend would accept the task in a merge queue for the repository:
 * committed work, nothing uncommitted, agent stopped, not merged yet.
 */
export function isQueueEligible(task: TaskDto, repoPath: string): boolean {
  return (
    task.repo_path === repoPath &&
    task.state !== "running" &&
    !task.merged_into &&
    task.commits_ahead > 0 &&
    (task.uncommitted_count ?? 0) === 0
  );
}

type Tone = "pending" | "active" | "merged" | "failed" | "muted";

const STATUS: Record<string, { label: string; tone: Tone; icon: () => ReactNode }> = {
  pending: { label: "Pending", tone: "pending", icon: PendingIcon },
  updating: { label: "Updating", tone: "active", icon: ProgressIcon },
  testing: { label: "Testing", tone: "active", icon: ProgressIcon },
  merging: { label: "Merging", tone: "active", icon: ProgressIcon },
  merged: { label: "Merged", tone: "merged", icon: CheckIcon },
  failed: { label: "Failed", tone: "failed", icon: FailedIcon },
  skipped: { label: "Skipped", tone: "muted", icon: SkippedIcon },
  cancelled: { label: "Cancelled", tone: "muted", icon: SkippedIcon },
  interrupted: { label: "Interrupted", tone: "muted", icon: SkippedIcon },
};

/** Unknown statuses from a newer backend still render, as plain text. */
function statusInfo(status: string) {
  return STATUS[status] ?? { label: status, tone: "pending" as Tone, icon: PendingIcon };
}

type Props = {
  /** The open repository; the queue only ever covers its tasks. */
  repoPath: string | null;
  tasks: TaskDto[] | null;
  entry: MergeQueueEntry | null;
  onStart: (repoPath: string, taskIds: string[]) => Promise<void>;
  onCancel: (repoPath: string) => Promise<void>;
  onDismiss: (repoPath: string) => void;
};

/** Bottom-panel view: choose tasks, order them, set the test command, and watch the queue. */
export default function MergeQueueView({ repoPath, ...rest }: Props) {
  if (!repoPath) {
    return <p className="wb-note wb-view-pad">Open a folder to merge its tasks in order.</p>;
  }
  // Keyed so choices and settings never leak from one repository to another.
  return <RepoMergeQueue key={repoPath} repoPath={repoPath} {...rest} />;
}

function RepoMergeQueue({ repoPath, tasks, entry, onStart, onCancel, onDismiss }: Props & { repoPath: string }) {
  const testCommand = useTestCommand(repoPath);
  const [order, setOrder] = useState<string[]>([]);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [confirming, setConfirming] = useState(false);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const moveButtons = useRef(new Map<string, HTMLButtonElement>());
  const pendingFocus = useRef<{ id: string; dir: "up" | "down" } | null>(null);

  const running = Boolean(entry?.queue.running);

  const { eligible, notReady } = useMemo(() => {
    const inRepo = (tasks ?? []).filter((t) => t.repo_path === repoPath && !t.merged_into);
    return {
      eligible: inRepo.filter((t) => isQueueEligible(t, repoPath)),
      notReady: inRepo.filter((t) => !isQueueEligible(t, repoPath)).length,
    };
  }, [tasks, repoPath]);

  // The user's arrangement first, then newly eligible tasks in list order.
  const ordered = useMemo(() => {
    const byId = new Map(eligible.map((t) => [t.id, t]));
    const kept = order.filter((id) => byId.has(id));
    const added = eligible.filter((t) => !kept.includes(t.id)).map((t) => t.id);
    return [...kept, ...added].map((id) => byId.get(id) as TaskDto);
  }, [eligible, order]);

  const queued = ordered.filter((t) => selected.has(t.id));
  const targetBranch = queued.find((t) => t.repo_branch)?.repo_branch;

  // Keep keyboard focus on the moved task, even when its button hits an edge.
  useEffect(() => {
    const pending = pendingFocus.current;
    if (!pending) return;
    pendingFocus.current = null;
    const same = moveButtons.current.get(`${pending.id}:${pending.dir}`);
    const other = moveButtons.current.get(`${pending.id}:${pending.dir === "up" ? "down" : "up"}`);
    (same && !same.disabled ? same : other)?.focus();
  });

  function move(task: TaskDto, dir: "up" | "down") {
    const ids = ordered.map((t) => t.id);
    const from = ids.indexOf(task.id);
    const to = dir === "up" ? from - 1 : from + 1;
    if (from < 0 || to < 0 || to >= ids.length) return;
    [ids[from], ids[to]] = [ids[to], ids[from]];
    pendingFocus.current = { id: task.id, dir };
    setOrder(ids);
    setAnnouncement(`${task.title} moved to position ${to + 1} of ${ids.length}.`);
  }

  function toggle(id: string, on: boolean) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  }

  async function openConfirm() {
    setStartError(null);
    // The queue reads the saved command, so an edit still in the field saves first.
    if (await testCommand.save()) setConfirming(true);
  }

  async function start() {
    setStarting(true);
    setStartError(null);
    try {
      await onStart(
        repoPath,
        queued.map((t) => t.id),
      );
    } catch (e) {
      setStartError(errorText(e));
    } finally {
      setStarting(false);
      setConfirming(false);
    }
  }

  const count = queued.length;
  const command = testCommand.saved;

  return (
    <div className="merge-queue">
      <div className="merge-queue__main">
      <section className="merge-queue__setup" aria-label="Set up merge queue">
        <label className="wb-field merge-queue__command">
          <span>Test command</span>
          <input
            type="text"
            className="mono"
            value={testCommand.value}
            placeholder="e.g. npm test - leave empty to merge without testing"
            disabled={!testCommand.loaded}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => testCommand.edit(e.target.value)}
            onBlur={() => void testCommand.save()}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void testCommand.save();
              }
            }}
          />
        </label>
        <p className="merge-queue__hint">
          Runs in each task&rsquo;s worktree after it is updated from the target branch. The queue stops at the first
          failure.{" "}
          <span className="merge-queue__saved" role="status">
            {testCommand.saving ? "Saving…" : testCommand.justSaved ? "Saved" : ""}
          </span>
        </p>
        {testCommand.error && (
          <p className="wb-note wb-note--error" role="alert">
            {testCommand.error}
          </p>
        )}

        <h3 className="wb-subhead">Ready to merge</h3>
        {tasks === null ? (
          <p className="wb-note" role="status">
            Loading tasks…
          </p>
        ) : ordered.length === 0 ? (
          <p className="wb-note">
            No tasks are ready. A task needs commits, no uncommitted changes, and a stopped agent.
          </p>
        ) : (
          <fieldset className="merge-queue__fields" disabled={running || confirming || starting}>
            <legend className="visually-hidden">Tasks to merge, in order</legend>
            <ol className="mq-picks" aria-label="Tasks ready to merge">
              {ordered.map((task, i) => {
                const position = queued.indexOf(task) + 1;
                return (
                  <li key={task.id} className={`mq-pick${position > 0 ? " mq-pick--on" : ""}`}>
                    <label className="mq-pick__main">
                      <input
                        type="checkbox"
                        checked={selected.has(task.id)}
                        onChange={(e) => toggle(task.id, e.target.checked)}
                      />
                      <span className="mq-pick__title">{task.title}</span>
                    </label>
                    <span className="mq-pick__pos" aria-hidden="true">
                      {position > 0 ? position : ""}
                    </span>
                    <span className="mq-pick__meta mono" title={task.branch}>
                      {task.branch}
                    </span>
                    <span className="mq-pick__meta">
                      {task.commits_ahead} {task.commits_ahead === 1 ? "commit" : "commits"}
                    </span>
                    <span className="mq-pick__moves">
                      <button
                        type="button"
                        className="wb-icon-btn"
                        aria-label={`Move ${task.title} up`}
                        title="Move up"
                        disabled={i === 0}
                        ref={(el) => {
                          if (el) moveButtons.current.set(`${task.id}:up`, el);
                          else moveButtons.current.delete(`${task.id}:up`);
                        }}
                        onClick={() => move(task, "up")}
                      >
                        <ArrowUpIcon />
                      </button>
                      <button
                        type="button"
                        className="wb-icon-btn"
                        aria-label={`Move ${task.title} down`}
                        title="Move down"
                        disabled={i === ordered.length - 1}
                        ref={(el) => {
                          if (el) moveButtons.current.set(`${task.id}:down`, el);
                          else moveButtons.current.delete(`${task.id}:down`);
                        }}
                        onClick={() => move(task, "down")}
                      >
                        <ArrowDownIcon />
                      </button>
                    </span>
                  </li>
                );
              })}
            </ol>
          </fieldset>
        )}
        <span className="visually-hidden" role="status">
          {announcement}
        </span>
        {notReady > 0 && (
          <p className="wb-note">
            {notReady} other {notReady === 1 ? "task isn’t" : "tasks aren’t"} ready: still running, uncommitted
            changes, or no commits.
          </p>
        )}
      </section>

      {entry && <QueueProgress entry={entry} />}
      </div>

        <div className="merge-queue__dock">
          {entry && (
            <div className="merge-queue__runhead">
              <p className="merge-queue__summary" role="status">
                {summarize(entry.queue.items, entry.queue.running)}
              </p>
              {entry.queue.running ? (
                <button
                  type="button"
                  className="wb-btn wb-btn--small"
                  disabled={entry.cancelling}
                  onClick={() => void onCancel(repoPath)}
                >
                  {entry.cancelling ? "Cancelling…" : "Cancel queue"}
                </button>
              ) : (
                <button type="button" className="wb-btn wb-btn--small" onClick={() => onDismiss(repoPath)}>
                  Dismiss
                </button>
              )}
            </div>
          )}
          {confirming ? (
            <div className="merge-queue__confirm" role="group" aria-label="Confirm merge queue">
              <p>
                Merge {count} {count === 1 ? "task" : "tasks"} into{" "}
                {targetBranch ? <span className="mono">{targetBranch}</span> : "your checked-out branch"}, in this
                order?{" "}
                {command ? (
                  <>
                    Tests run first: <span className="mono">{command}</span>
                  </>
                ) : (
                  "No test command is set, so tasks merge without testing."
                )}
              </p>
              <div className="merge-queue__actions">
                <button type="button" className="wb-btn wb-btn--primary" disabled={starting} onClick={() => void start()}>
                  {starting ? "Starting…" : "Confirm merge"}
                </button>
                <button type="button" className="wb-btn" disabled={starting} onClick={() => setConfirming(false)}>
                  Cancel
                </button>
              </div>
            </div>
          ) : (
            <div className="merge-queue__actions">
              <button
                type="button"
                className="wb-btn wb-btn--primary"
                // Not disabled while saving: blurring the field starts a save between
                // mousedown and click, and openConfirm waits for it anyway.
                disabled={count === 0 || running}
                onClick={() => void openConfirm()}
              >
                Merge {count} in order
              </button>
              {running && <span className="merge-queue__hint">A queue is running for this repository.</span>}
            </div>
          )}
          {startError && (
            <p className="wb-note wb-note--error" role="alert">
              {startError}
            </p>
          )}
        </div>
    </div>
  );
}

function QueueProgress({ entry }: { entry: MergeQueueEntry }) {
  const { queue, error } = entry;
  return (
    <section className="merge-queue__run" aria-label="Merge queue progress">
      <p className="merge-queue__hint">
        {queue.test_command ? (
          <>
            Tested with <span className="mono">{queue.test_command}</span>
          </>
        ) : (
          "Merging without a test command"
        )}
      </p>
      {error && (
        <p className="wb-note wb-note--error" role="alert">
          {error}
        </p>
      )}
      <ol className="mq-items" aria-label="Merge queue">
        {queue.items.map((item) => (
          <QueueItem key={item.task_id} item={item} />
        ))}
      </ol>
    </section>
  );
}

function QueueItem({ item }: { item: MergeQueueItemDto }) {
  const info = statusInfo(item.status);
  const Icon = info.icon;
  const detail = item.detail?.trim() || null;
  return (
    <li className={`mq-item mq-item--${info.tone}`} aria-label={item.title}>
      <span className="mq-item__head">
        <span className="mq-item__icon">
          <Icon />
        </span>
        <span className="mq-item__status">{info.label}</span>
        <span className="mq-item__title">{item.title}</span>
        {detail && info.tone !== "failed" && (
          <span className={`mq-item__detail${item.status === "testing" ? " mono" : ""}`} title={detail}>
            {detail}
          </span>
        )}
      </span>
      {detail && info.tone === "failed" && (
        <details className="mq-item__failure">
          <summary>{detail.split("\n")[0]}</summary>
          <pre className="mq-item__output">{detail}</pre>
        </details>
      )}
    </li>
  );
}

/** One line on where the queue is, for sighted users and screen readers alike. */
function summarize(items: MergeQueueItemDto[], running: boolean): string {
  const count = (status: string) => items.filter((i) => i.status === status).length;
  if (running) {
    const index = items.findIndex((i) => statusInfo(i.status).tone === "active");
    const current = index >= 0 ? items[index] : items.find((i) => i.status === "pending");
    if (!current) return "Merge queue running";
    const step = statusInfo(current.status).label.toLowerCase();
    return `Merge queue running: ${items.indexOf(current) + 1} of ${items.length}, ${step} ${current.title}`;
  }
  const parts = (["merged", "failed", "skipped", "cancelled", "interrupted"] as const)
    .map((status) => [status, count(status)] as const)
    .filter(([, n]) => n > 0)
    .map(([status, n]) => `${n} ${status}`);
  const verb =
    count("interrupted") > 0
      ? "interrupted"
      : count("cancelled") > 0
        ? "cancelled"
        : count("failed") > 0
          ? "stopped"
          : "finished";
  return `Merge queue ${verb}${parts.length > 0 ? `: ${parts.join(", ")}` : ""}`;
}

/**
 * The repository's test command: loads once, saves on demand (blur, Enter, or
 * before a queue starts), and serializes saves so the last edit wins.
 */
function useTestCommand(repoPath: string) {
  const [value, setValue] = useState("");
  const [saved, setSaved] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [justSaved, setJustSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const valueRef = useRef(value);
  const savedRef = useRef<string | null>(null);
  const loadedRef = useRef(false);
  const mountedRef = useRef(true);
  const chainRef = useRef<Promise<boolean>>(Promise.resolve(true));

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    getRepoSettings(repoPath)
      .then((settings) => {
        savedRef.current = settings.test_command;
        valueRef.current = settings.test_command ?? "";
        loadedRef.current = true;
        if (!mountedRef.current) return;
        setSaved(settings.test_command);
        setValue(settings.test_command ?? "");
        setLoaded(true);
      })
      .catch((e) => {
        // Still editable: saving a command replaces whatever could not be read.
        loadedRef.current = true;
        if (!mountedRef.current) return;
        setError(`Could not load the test command: ${errorText(e)}`);
        setLoaded(true);
      });
  }, [repoPath]);

  const edit = useCallback((next: string) => {
    valueRef.current = next;
    setValue(next);
    setJustSaved(false);
  }, []);

  const save = useCallback((): Promise<boolean> => {
    const run = chainRef.current.then(async () => {
      const next = valueRef.current.trim() || null;
      if (!loadedRef.current || next === savedRef.current) return true;
      setSaving(true);
      setError(null);
      try {
        const settings = await setRepoTestCommand(repoPath, next);
        savedRef.current = settings.test_command;
        if (!mountedRef.current) return true;
        setSaved(settings.test_command);
        // Show the stored (trimmed) form unless the user has typed since.
        if ((valueRef.current.trim() || null) === next) {
          valueRef.current = settings.test_command ?? "";
          setValue(settings.test_command ?? "");
        }
        setJustSaved(true);
        return true;
      } catch (e) {
        if (mountedRef.current) setError(errorText(e));
        return false;
      } finally {
        if (mountedRef.current) setSaving(false);
      }
    });
    chainRef.current = run;
    return run;
  }, [repoPath]);

  return { value, saved, loaded, saving, justSaved, error, edit, save };
}
