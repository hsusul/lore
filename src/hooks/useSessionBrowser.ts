import { useCallback, useRef, useState } from "react";

import {
  exportSessionMarkdown,
  forgetSession,
  getGitSnapshot,
  getSession,
  listFolderSessionsPage,
  listRepositorySessionsPage,
  listSessionsPage,
  saveSessionExport,
  sessionSecretCount,
  type GitObservationDto,
  type SessionDetail,
  type SessionSummary,
} from "../ipc";

/** Sessions fetched per browse page. Initial navigation stays bounded. */
export const SESSION_PAGE = 100;

export interface UseSessionBrowserOptions {
  onError: (error: string) => void;
  onNotice: (notice: string) => void;
  onClearError?: () => void;
  onClearNotice?: () => void;
}

export function useSessionBrowser({
  onError,
  onNotice,
  onClearError,
  onClearNotice,
}: UseSessionBrowserOptions) {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [sessionCursor, setSessionCursor] = useState<string | null>(null);
  const [loadingMoreSessionsRequest, setLoadingMoreSessionsRequest] = useState<number | null>(null);
  const loadingMoreSessions = loadingMoreSessionsRequest !== null;

  const [selectedSession, setSelectedSession] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [git, setGit] = useState<GitObservationDto[]>([]);
  const [secretCount, setSecretCount] = useState(0);

  const sessionsRequestRef = useRef(0);
  const sessionsPendingRequestRef = useRef<number | null>(null);
  const loadedSessionCountRef = useRef(0);
  const sessionRequestRef = useRef(0);
  const sessionPendingRequestRef = useRef<number | null>(null);

  const loadSessions = useCallback(
    async (repo: string | null, folder: string | null, showPending = false) => {
      const request = ++sessionsRequestRef.current;
      setLoadingMoreSessionsRequest(null);
      const limit = showPending
        ? SESSION_PAGE
        : Math.max(SESSION_PAGE, loadedSessionCountRef.current);
      if (showPending) {
        sessionsPendingRequestRef.current = request;
        loadedSessionCountRef.current = 0;
        setSessions([]);
        setSessionCursor(null);
      }
      try {
        const page = folder
          ? await listFolderSessionsPage(folder, limit, null)
          : repo
            ? await listRepositorySessionsPage(repo, limit, null)
            : await listSessionsPage(limit, null);
        if (request === sessionsRequestRef.current) {
          // The newest request owns the pane and also settles any pending marker
          // inherited from a manual load that it superseded.
          sessionsPendingRequestRef.current = null;
          loadedSessionCountRef.current = page.sessions.length;
          setSessions(page.sessions);
          setSessionCursor(page.next_cursor);
        }
      } catch (e) {
        if (request !== sessionsRequestRef.current) return;
        sessionsPendingRequestRef.current = null;
        loadedSessionCountRef.current = 0;
        setSessions([]);
        setSessionCursor(null);
        throw e;
      }
    },
    [],
  );

  const loadOlderSessions = useCallback(
    async (repo: string | null, folder: string | null) => {
      if (sessionCursor === null || loadingMoreSessionsRequest !== null) return;
      const request = ++sessionsRequestRef.current;
      const forCursor = sessionCursor;
      setLoadingMoreSessionsRequest(request);
      onClearError?.();
      try {
        const page = folder
          ? await listFolderSessionsPage(folder, SESSION_PAGE, forCursor)
          : repo
            ? await listRepositorySessionsPage(repo, SESSION_PAGE, forCursor)
            : await listSessionsPage(SESSION_PAGE, forCursor);
        if (request !== sessionsRequestRef.current) return;
        // A cursor page should be disjoint, but deduping at the UI boundary keeps
        // a concurrent archive refresh from ever painting a duplicate.
        const seen = new Set(sessions.map((session) => session.id));
        const merged = [
          ...sessions,
          ...page.sessions.filter((session) => !seen.has(session.id)),
        ];
        loadedSessionCountRef.current = merged.length;
        setSessions(merged);
        setSessionCursor(page.next_cursor);
      } catch (e) {
        if (request === sessionsRequestRef.current) onError(String(e));
      } finally {
        setLoadingMoreSessionsRequest((current) => (current === request ? null : current));
      }
    },
    [loadingMoreSessionsRequest, onClearError, onError, sessionCursor, sessions],
  );

  const openSession = useCallback(
    async (id: string) => {
      const request = ++sessionRequestRef.current;
      sessionPendingRequestRef.current = request;
      setSelectedSession(id);
      onClearError?.();
      onClearNotice?.();
      try {
        const [loaded, snapshot, secrets] = await Promise.all([
          getSession(id),
          getGitSnapshot(id),
          sessionSecretCount(id),
        ]);
        if (request !== sessionRequestRef.current) return;
        sessionPendingRequestRef.current = null;
        setDetail(loaded);
        setGit(snapshot);
        setSecretCount(secrets);
      } catch (e) {
        if (request !== sessionRequestRef.current) return;
        sessionPendingRequestRef.current = null;
        setSelectedSession(null);
        setDetail(null);
        onError(String(e));
      }
    },
    [onClearError, onClearNotice, onError],
  );

  const handleExport = useCallback(async () => {
    if (!selectedSession) return;
    try {
      const markdown = await exportSessionMarkdown(selectedSession, false);
      if (markdown == null) return;
      if (!navigator.clipboard?.writeText) {
        onError("Clipboard is unavailable here. Use “Save file” instead.");
        return;
      }
      await navigator.clipboard.writeText(markdown);
      onNotice("Copied Markdown to the clipboard (flagged secrets redacted).");
    } catch (e) {
      onError(String(e));
    }
  }, [onError, onNotice, selectedSession]);

  const handleSaveFile = useCallback(async () => {
    if (!selectedSession) return;
    try {
      const saved = await saveSessionExport(selectedSession, null, false);
      if (saved) {
        onNotice("Saved session export.");
      }
    } catch (e) {
      onError(String(e));
    }
  }, [onError, onNotice, selectedSession]);

  const handleForget = useCallback(
    async (onRefresh: () => Promise<void>) => {
      if (!selectedSession) return;
      if (!window.confirm("Forget this session? This permanently removes it from Lore.")) {
        return;
      }
      try {
        sessionRequestRef.current += 1;
        sessionPendingRequestRef.current = null;
        const report = await forgetSession(selectedSession);
        setDetail(null);
        setSelectedSession(null);
        await onRefresh();
        const remaining =
          report.source_paths.length > 0
            ? ` The original agent log(s) remain: ${report.source_paths.join(", ")}.`
            : "";
        onNotice(`Session forgotten (${report.blobs_removed} blob(s) removed).${remaining}`);
      } catch (e) {
        onError(String(e));
      }
    },
    [onError, onNotice, selectedSession],
  );

  const resetForgetAll = useCallback(() => {
    sessionRequestRef.current += 1;
    sessionPendingRequestRef.current = null;
    setDetail(null);
    setSelectedSession(null);
    setGit([]);
  }, []);

  return {
    sessions,
    setSessions,
    sessionCursor,
    setSessionCursor,
    loadingMoreSessions,
    selectedSession,
    setSelectedSession,
    detail,
    setDetail,
    git,
    setGit,
    secretCount,
    sessionsPendingRequestRef,
    sessionPendingRequestRef,
    loadSessions,
    loadOlderSessions,
    openSession,
    handleExport,
    handleSaveFile,
    handleForget,
    resetForgetAll,
  };
}
