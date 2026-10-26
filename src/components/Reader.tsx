import { useRef, useState, type RefObject } from "react";

import { agentLabel, formatRelative, formatTime, formatTokens } from "../format";
import type { FolderSummary, RepositorySummary } from "../ipc";
import type { OpenSession, OpenSessionState } from "../hooks/useOpenSession";
import ErrorBoundary from "./ErrorBoundary";
import FileChanges from "./FileChanges";
import GitEvidence from "./GitEvidence";
import Transcript from "./Transcript";

export interface ReaderActions {
  copyMarkdown: (id: string) => void;
  saveFile: (id: string) => void;
  file: (id: string, folderId: string | null) => void;
  forget: (id: string, title: string) => void;
}

type Tab = "conversation" | "files" | "git";

function duration(start: number | null, end: number | null): string | null {
  if (start == null || end == null || end <= start) return null;
  const minutes = Math.round((end - start) / 60_000);
  if (minutes < 1) return "under a minute";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  return `${hours} h ${minutes % 60} min`;
}

function ParseNotice({ session }: { session: OpenSession }) {
  const { summary, parse_note, file_events } = session.detail;
  if (summary.parse_status === "ok") return null;
  const failed = summary.parse_status === "failed";
  return (
    <section className={`notice notice--${failed ? "danger" : "warn"}`} aria-labelledby="parse-heading">
      <h3 id="parse-heading" className="notice__title">
        {failed ? "Lore could not fully read this session" : "Lore read part of this session"}
      </h3>
      <p>
        Recovered {summary.message_count} messages, {summary.tool_call_count} tool calls, and{" "}
        {file_events.length} file changes{session.git.length > 0 ? ", with git context" : ", with no git context"}.
        Your original agent log is untouched, so a later Lore can read it again.
      </p>
      <p>
        Parser note:{" "}
        <code>
          {parse_note ??
            (failed
              ? "Lore could not safely normalize this source format."
              : "Lore skipped records it could not safely normalize.")}
        </code>
      </p>
    </section>
  );
}

function FolderSelect({
  folders,
  onFile,
}: {
  folders: FolderSummary[];
  onFile: (folderId: string | null) => void;
}) {
  return (
    <select
      className="select"
      aria-label="File in folder"
      value=""
      onChange={(event) => {
        const value = event.target.value;
        if (value === "") return;
        onFile(value === "__none" ? null : value);
      }}
    >
      <option value="">File in folder…</option>
      {folders.map((folder) => (
        <option key={folder.id} value={folder.id}>
          {folder.name}
        </option>
      ))}
      <option value="__none">Remove from its folder</option>
    </select>
  );
}

function SessionReader({
  session,
  terms,
  folders,
  repositories,
  actions,
  onBack,
  readerRef,
}: {
  session: OpenSession;
  terms: string[];
  folders: FolderSummary[];
  repositories: RepositorySummary[];
  actions: ReaderActions;
  onBack: () => void;
  readerRef: RefObject<HTMLElement>;
}) {
  const [tab, setTab] = useState<Tab>("conversation");
  const tabRefs = useRef<Record<Tab, HTMLButtonElement | null>>({ conversation: null, files: null, git: null });
  const { summary, segments, file_events, messages, next_message_cursor } = session.detail;
  const title = summary.title ?? "Untitled session";
  const tokens = (summary.total_input_tokens ?? 0) + (summary.total_output_tokens ?? 0);
  const length = duration(summary.started_at, summary.ended_at);
  const tabs: { id: Tab; label: string; count?: number }[] = [
    { id: "conversation", label: "Conversation" },
    { id: "files", label: "Files", count: file_events.length },
    { id: "git", label: "Git", count: session.git.length },
  ];

  function onTabKey(event: React.KeyboardEvent) {
    const index = tabs.findIndex((t) => t.id === tab);
    const delta = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    if (delta === 0) return;
    event.preventDefault();
    const next = tabs[(index + delta + tabs.length) % tabs.length].id;
    setTab(next);
    tabRefs.current[next]?.focus();
  }

  return (
    <article
      ref={readerRef}
      className="reader"
      aria-labelledby="reader-title"
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === "Escape" && !event.defaultPrevented) {
          event.preventDefault();
          onBack();
        }
      }}
    >
      <header className="reader__head">
        <h2 id="reader-title" className="reader__title">
          {title}
        </h2>
        <dl className="facts">
          <div>
            <dt>Agent</dt>
            <dd>{agentLabel(summary.agent_id)}</dd>
          </div>
          {summary.primary_model && (
            <div>
              <dt>Model</dt>
              <dd>{summary.primary_model}</dd>
            </div>
          )}
          {summary.started_at != null && (
            <div>
              <dt>Started</dt>
              <dd title={formatTime(summary.started_at)}>{formatRelative(summary.started_at)}</dd>
            </div>
          )}
          {length && (
            <div>
              <dt>Length</dt>
              <dd>{length}</dd>
            </div>
          )}
          <div>
            <dt>Messages</dt>
            <dd>{Math.max(summary.message_count, messages.length)}</dd>
          </div>
          <div>
            <dt>Tool calls</dt>
            <dd>{summary.tool_call_count}</dd>
          </div>
          {tokens > 0 && (
            <div>
              <dt>Tokens</dt>
              <dd
                title={`${(summary.total_input_tokens ?? 0).toLocaleString()} in, ${(summary.total_output_tokens ?? 0).toLocaleString()} out${summary.total_cache_tokens ? `, ${summary.total_cache_tokens.toLocaleString()} cached` : ""}`}
              >
                {formatTokens(tokens)}
              </dd>
            </div>
          )}
          {summary.est_cost_usd != null && summary.est_cost_usd > 0 && (
            <div>
              <dt>Est. cost</dt>
              <dd>${summary.est_cost_usd.toFixed(2)}</dd>
            </div>
          )}
          {segments.length > 1 && (
            <div>
              <dt>Contexts</dt>
              <dd>{segments.length}</dd>
            </div>
          )}
        </dl>
        {session.secrets > 0 && (
          <p className="flag-line">
            <span className="flag flag--warn">
              {session.secrets === 1 ? "1 secret flagged" : `${session.secrets} secrets flagged`}
            </span>{" "}
            Redacted from search and exports. The original log still has them.
          </p>
        )}
        <div className="reader__actions">
          <button type="button" className="btn" onClick={() => actions.copyMarkdown(summary.id)}>
            Copy as Markdown
          </button>
          <button type="button" className="btn" onClick={() => actions.saveFile(summary.id)}>
            Save as file…
          </button>
          <FolderSelect folders={folders} onFile={(folderId) => actions.file(summary.id, folderId)} />
          <span className="reader__spacer" />
          <button type="button" className="btn btn--danger" onClick={() => actions.forget(summary.id, title)}>
            Forget…
          </button>
        </div>
      </header>

      <ParseNotice session={session} />

      <div className="tabs" role="tablist" aria-label="Session views" onKeyDown={onTabKey}>
        {tabs.map((t) => (
          <button
            key={t.id}
            ref={(el) => {
              tabRefs.current[t.id] = el;
            }}
            type="button"
            role="tab"
            id={`tab-${t.id}`}
            aria-selected={tab === t.id}
            aria-controls={`panel-${t.id}`}
            tabIndex={tab === t.id ? 0 : -1}
            className="tabs__tab"
            onClick={() => setTab(t.id)}
          >
            {t.label}
            {t.count !== undefined && <span className="tabs__count">{t.count}</span>}
          </button>
        ))}
      </div>
      <div className="reader__panel" role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
        <ErrorBoundary>
          {tab === "conversation" && (
            <Transcript
              sessionId={summary.id}
              agentId={summary.agent_id}
              initial={messages}
              initialCursor={next_message_cursor}
              total={summary.message_count}
              terms={terms}
            />
          )}
          {tab === "files" && <FileChanges files={file_events} />}
          {tab === "git" && (
            <GitEvidence observations={session.git} segments={segments} repositories={repositories} />
          )}
        </ErrorBoundary>
      </div>
    </article>
  );
}

export default function Reader({
  state,
  onRetry,
  ...props
}: {
  state: OpenSessionState;
  onRetry: () => void;
  terms: string[];
  folders: FolderSummary[];
  repositories: RepositorySummary[];
  actions: ReaderActions;
  onBack: () => void;
  readerRef: RefObject<HTMLElement>;
}) {
  switch (state.status) {
    case "none":
      return (
        <div className="reader-empty">
          <p className="reader-empty__lead">Pick a session to read it.</p>
          <p className="quiet">
            <kbd>↑</kbd> <kbd>↓</kbd> move through the list, <kbd>Enter</kbd> opens, <kbd>Esc</kbd> comes back.
          </p>
        </div>
      );
    case "loading":
      return (
        <div className="reader-empty" role="status">
          <p className="quiet">Opening session…</p>
        </div>
      );
    case "missing":
      return (
        <div className="reader-empty" role="status">
          <p className="reader-empty__lead">This session is no longer in the archive.</p>
          <p className="quiet">It may have been forgotten. Pick another one from the list.</p>
        </div>
      );
    case "failed":
      return (
        <div className="reader-empty" role="alert">
          <p className="reader-empty__lead">Lore could not open this session.</p>
          <p className="quiet">{state.error}</p>
          <button type="button" className="btn" onClick={onRetry}>
            Try again
          </button>
        </div>
      );
    case "ready":
      return <SessionReader key={state.id} session={state.session} {...props} />;
  }
}
