import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { cancelMergeQueue, getMergeQueue, startMergeQueue, type MergeQueueDto } from "../../ipc";
import { errorText } from "./state";

/** How often a running queue is re-read. */
export const MERGE_QUEUE_POLL_MS = 1000;

export type MergeQueueEntry = {
  queue: MergeQueueDto;
  /** Cancel was requested; the queue stops after its current step. */
  cancelling: boolean;
  /** The last poll or cancel failed; polling continues. */
  error: string | null;
};

export type MergeQueuesHandle = {
  /** Queues this window has seen running, keyed by the repository path the UI uses. */
  entries: Record<string, MergeQueueEntry>;
  isRunning: (repoPath: string) => boolean;
  /** Start a queue; rejects with the backend's reason. */
  start: (repoPath: string, taskIds: string[]) => Promise<void>;
  cancel: (repoPath: string) => Promise<void>;
  /** Forget a finished queue's results. */
  dismiss: (repoPath: string) => void;
};

/**
 * Merge queues per repository. Lives in the workbench, so polling continues when
 * the panel closes or another repository is opened. A running queue is polled
 * every second (never overlapping) until it reports `running: false`; its
 * results then stay until dismissed or replaced by a new queue. Opening a
 * repository picks up a queue that is already running there.
 */
export function useMergeQueues(workspace: string | null, onFinished: () => void): MergeQueuesHandle {
  const [entries, setEntries] = useState<Record<string, MergeQueueEntry>>({});
  const onFinishedRef = useRef(onFinished);
  onFinishedRef.current = onFinished;

  // A queue may already be running in the repository being opened.
  useEffect(() => {
    if (!workspace) return;
    let cancelled = false;
    getMergeQueue(workspace)
      .then((queue) => {
        if (cancelled || !queue?.running) return;
        setEntries((prev) =>
          prev[workspace]?.queue.running
            ? prev
            : { ...prev, [workspace]: { queue, cancelling: false, error: null } },
        );
      })
      .catch(() => {
        // Nothing to show; starting a queue reports its own errors.
      });
    return () => {
      cancelled = true;
    };
  }, [workspace]);

  const runningKey = Object.keys(entries)
    .filter((repo) => entries[repo].queue.running)
    .sort()
    .join("\n");

  useEffect(() => {
    if (!runningKey) return;
    const repos = runningKey.split("\n");
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    async function tick() {
      const results = await Promise.all(
        repos.map((repo) =>
          getMergeQueue(repo).then(
            (queue) => ({ repo, queue, error: null }),
            (e: unknown) => ({ repo, queue: undefined, error: errorText(e) }),
          ),
        ),
      );
      if (cancelled) return;
      const finished = results.some((r) => r.queue === null || r.queue?.running === false);
      setEntries((prev) => {
        const next = { ...prev };
        for (const { repo, queue, error } of results) {
          const entry = next[repo];
          if (!entry) continue;
          if (queue === null) {
            // The backend no longer knows this queue; stop showing it as running.
            delete next[repo];
          } else if (queue === undefined) {
            next[repo] = { ...entry, error };
          } else {
            next[repo] = { queue, cancelling: queue.running && entry.cancelling, error: null };
          }
        }
        return next;
      });
      if (finished) onFinishedRef.current();
      // A finished repository changes `runningKey`, which restarts this effect.
      if (!cancelled && !finished) timer = setTimeout(() => void tick(), MERGE_QUEUE_POLL_MS);
    }

    timer = setTimeout(() => void tick(), MERGE_QUEUE_POLL_MS);
    return () => {
      cancelled = true;
      if (timer !== null) clearTimeout(timer);
    };
  }, [runningKey]);

  const start = useCallback(async (repoPath: string, taskIds: string[]) => {
    const queue = await startMergeQueue(repoPath, taskIds);
    setEntries((prev) => ({ ...prev, [repoPath]: { queue, cancelling: false, error: null } }));
  }, []);

  const cancel = useCallback(async (repoPath: string) => {
    const update = (patch: Partial<MergeQueueEntry>) =>
      setEntries((prev) => (prev[repoPath] ? { ...prev, [repoPath]: { ...prev[repoPath], ...patch } } : prev));
    update({ cancelling: true, error: null });
    try {
      await cancelMergeQueue(repoPath);
    } catch (e) {
      update({ cancelling: false, error: errorText(e) });
    }
  }, []);

  const dismiss = useCallback((repoPath: string) => {
    setEntries((prev) => {
      if (!prev[repoPath] || prev[repoPath].queue.running) return prev;
      const next = { ...prev };
      delete next[repoPath];
      return next;
    });
  }, []);

  return useMemo(
    () => ({
      entries,
      isRunning: (repoPath: string) => Boolean(entries[repoPath]?.queue.running),
      start,
      cancel,
      dismiss,
    }),
    [entries, start, cancel, dismiss],
  );
}
