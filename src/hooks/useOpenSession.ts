import { useCallback, useEffect, useRef, useState } from "react";

import {
  getGitSnapshot,
  getSession,
  sessionSecretCount,
  type GitObservationDto,
  type SessionDetail,
} from "../ipc";

export interface OpenSession {
  detail: SessionDetail;
  git: GitObservationDto[];
  secrets: number;
}

export type OpenSessionState =
  | { status: "none" }
  | { status: "loading"; id: string }
  | { status: "missing"; id: string }
  | { status: "failed"; id: string; error: string }
  | { status: "ready"; id: string; session: OpenSession };

/** Load the reader's session. Only the latest requested id may paint. */
export function useOpenSession(id: string | null) {
  const [state, setState] = useState<OpenSessionState>({ status: "none" });
  const [attempt, setAttempt] = useState(0);
  const request = useRef(0);

  useEffect(() => {
    const req = ++request.current;
    if (id === null) {
      setState({ status: "none" });
      return;
    }
    setState({ status: "loading", id });
    Promise.all([getSession(id), getGitSnapshot(id), sessionSecretCount(id)]).then(
      ([detail, git, secrets]) => {
        if (req !== request.current) return;
        setState(
          detail
            ? { status: "ready", id, session: { detail, git, secrets } }
            : { status: "missing", id },
        );
      },
      (e: unknown) => {
        if (req !== request.current) return;
        setState({ status: "failed", id, error: String(e) });
      },
    );
  }, [id, attempt]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  return { state, retry };
}
