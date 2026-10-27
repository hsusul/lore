import { Info } from "lucide-react";
import { useCallback, useEffect, useMemo, useState, type RefObject } from "react";

import { agentLabel, formatDayAndTime, formatDuration, formatTime, plural } from "../format";
import { listSessionMessagesPage, type MessageDto, type RepositorySummary } from "../ipc";
import type { OpenSession, OpenSessionState } from "../hooks/useOpenSession";
import type { ToolCallMode } from "../hooks/useToolCallMode";
import ErrorBoundary from "./ErrorBoundary";
import FileChanges from "./FileChanges";
import GitEvidence from "./GitEvidence";
import Transcript, { type MoreMessages } from "./Transcript";
import { buildTranscript, callsByFile, callsOf, fileByCall } from "./transcript-model";
import { Button, ICON } from "./ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "./ui/popover";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "./ui/tabs";
import { Tooltip } from "./ui/tooltip";

const PAGE = 200;

type Tab = "conversation" | "files" | "git";
const TAB_KEYS: Record<string, Tab> = { "1": "conversation", "2": "files", "3": "git" };

/** The metadata line under the title (spec 9.3, 10): one run of clauses joined with " · ". */
function metadata(session: OpenSession, repositories: RepositorySummary[]): string {
  const { summary, segments } = session.detail;
  const repoIds = [...new Set(segments.map((s) => s.repository_id).filter((id): id is string => id !== null))];
  const repos = repoIds.map((id) => repositories.find((r) => r.id === id)?.display_name ?? id).join(", ");
  return [
    agentLabel(summary.agent_id),
    summary.primary_model,
    summary.started_at !== null ? formatDayAndTime(summary.started_at) : null,
    formatDuration(summary.started_at, summary.ended_at),
    repos || null,
    summary.parse_status === "partial" ? "Partially parsed" : summary.parse_status === "failed" ? "Not fully parsed" : null,
    session.secrets > 0 ? `${plural(session.secrets, "secret")} redacted` : null,
  ]
    .filter((clause): clause is string => !!clause)
    .join(" · ");
}

/** Counts, tokens, cost and the parser's note, behind the info button (spec 9.3). Never a grid on the page. */
function Details({ session, loaded }: { session: OpenSession; loaded: number }) {
  const { summary, parse_note } = session.detail;
  const n = (x: number) => x.toLocaleString();
  const input = summary.total_input_tokens ?? 0;
  const output = summary.total_output_tokens ?? 0;
  const parse =
    summary.parse_status === "ok"
      ? "Parsed completely."
      : `${summary.parse_status === "failed" ? "Lore could not fully read this session" : "Lore read part of this session"}${
          parse_note ? `: ${parse_note}.` : "."
        } The original log is untouched, so a later Lore can read it again.`;
  const secrets =
    session.secrets > 0
      ? `${plural(session.secrets, "secret")} redacted from search and exports. The original log still has them.`
      : "No secrets flagged.";
  return (
    <Popover>
      <Tooltip label="Details">
        <PopoverTrigger asChild>
          <Button variant="icon" aria-label="Details">
            <Info className={ICON.small} aria-hidden="true" />
          </Button>
        </PopoverTrigger>
      </Tooltip>
      <PopoverContent align="end" aria-labelledby="details-title">
        <h3 id="details-title" className="mb-2 text-headline">
          Details
        </h3>
        <dl className="grid grid-cols-facts gap-x-3 gap-y-1 text-callout">
          <dt className="text-label-2">Messages</dt>
          <dd className="tabular-nums">
            {n(Math.max(summary.message_count, loaded))}, including {plural(summary.tool_call_count, "tool call")}
          </dd>
          {input + output > 0 && (
            <>
              <dt className="text-label-2">Tokens</dt>
              <dd className="tabular-nums">
                {n(input)} in, {n(output)} out{summary.total_cache_tokens ? `, ${n(summary.total_cache_tokens)} cached` : ""}
              </dd>
            </>
          )}
          {summary.est_cost_usd != null && summary.est_cost_usd > 0 && (
            <>
              <dt className="text-label-2">Cost</dt>
              <dd className="tabular-nums">${summary.est_cost_usd.toFixed(2)}, estimated</dd>
            </>
          )}
        </dl>
        <p className="mt-2 text-callout text-label-2">
          {parse} {secrets}
        </p>
      </PopoverContent>
    </Popover>
  );
}

interface MessageLog {
  messages: MessageDto[];
  cursor: string | null;
  status: MoreMessages["status"];
}

function SessionReader({
  session,
  terms,
  repositories,
  toolCalls,
  onBack,
  readerRef,
}: {
  session: OpenSession;
  terms: string[];
  repositories: RepositorySummary[];
  toolCalls: ToolCallMode;
  onBack: () => void;
  readerRef: RefObject<HTMLElement>;
}) {
  const { summary, segments, file_events } = session.detail;
  const title = summary.title ?? "Untitled session";
  const [tab, setTab] = useState<Tab>("conversation");
  const [reveal, setReveal] = useState<string | null>(null);
  const [log, setLog] = useState<MessageLog>({
    messages: session.detail.messages,
    cursor: session.detail.next_message_cursor,
    status: "idle",
  });

  // The Files tab matches file events to the calls in the loaded transcript.
  const blocks = useMemo(() => buildTranscript(log.messages), [log.messages]);
  const byFile = useMemo(() => callsByFile(file_events, callsOf(blocks)), [blocks, file_events]);
  const byCall = useMemo(() => fileByCall(file_events, byFile), [byFile, file_events]);
  const fileFor = useCallback((call: { key: string }) => byCall.get(call.key) ?? null, [byCall]);
  const revealed = useCallback(() => setReveal(null), []);

  async function loadMore() {
    if (log.cursor === null || log.status === "loading") return;
    setLog((prev) => ({ ...prev, status: "loading" }));
    try {
      const page = await listSessionMessagesPage(summary.id, PAGE, log.cursor);
      setLog((prev) => ({ messages: [...prev.messages, ...page.messages], cursor: page.next_cursor, status: "idle" }));
    } catch {
      setLog((prev) => ({ ...prev, status: "failed" }));
    }
  }

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const next = TAB_KEYS[event.key];
      if (!next || !(event.metaKey || event.ctrlKey) || event.shiftKey || event.altKey) return;
      event.preventDefault();
      setTab(next);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const tabs: { id: Tab; label: string; count: number }[] = [
    { id: "conversation", label: "Conversation", count: 0 },
    { id: "files", label: "Files", count: file_events.length },
    { id: "git", label: "Git", count: 0 },
  ];

  return (
    <article
      ref={readerRef}
      className="px-6 pb-8 pt-5 outline-none"
      aria-labelledby="reader-title"
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === "Escape" && !event.defaultPrevented) {
          event.preventDefault();
          onBack();
        }
      }}
    >
      <Tabs value={tab} onValueChange={(value) => setTab(value as Tab)}>
        <div>
          <h2 id="reader-title" className="line-clamp-2 max-w-measure text-title2 text-label">
            {title}
          </h2>
          <div className="mt-1 flex min-w-0 items-center gap-1 text-callout text-label-2">
            <p className="min-w-0 truncate" title={summary.started_at !== null ? formatTime(summary.started_at) : undefined}>
              {metadata(session, repositories)}
            </p>
            <Details session={session} loaded={log.messages.length} />
          </div>
          <TabsList aria-label="Session views" className="mt-3">
            {tabs.map((t, index) => (
              <TabsTrigger key={t.id} value={t.id} aria-keyshortcuts={`Meta+${index + 1}`}>
                {t.label}
                {t.count > 0 && <span className="font-regular tabular-nums text-label-2">{` ${t.count}`}</span>}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>
        <ErrorBoundary>
          <TabsContent value="conversation" className="mt-5">
            <Transcript
              blocks={blocks}
              agentId={summary.agent_id}
              mode={toolCalls}
              terms={terms}
              fileFor={fileFor}
              more={log.cursor === null ? null : { status: log.status, onMore: () => void loadMore() }}
              reveal={reveal}
              onRevealed={revealed}
              key={toolCalls}
            />
          </TabsContent>
          <TabsContent value="files" className="mt-5">
            <FileChanges
              files={file_events}
              callsByFile={byFile}
              onShowCall={(key) => {
                setReveal(key);
                setTab("conversation");
              }}
            />
          </TabsContent>
          <TabsContent value="git" className="mt-5">
            <GitEvidence observations={session.git} segments={segments} repositories={repositories} summary={summary} />
          </TabsContent>
        </ErrorBoundary>
      </Tabs>
    </article>
  );
}

function Centered({ role, children }: { role?: "status" | "alert"; children: React.ReactNode }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center" role={role}>
      {children}
    </div>
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
  repositories: RepositorySummary[];
  toolCalls: ToolCallMode;
  onBack: () => void;
  readerRef: RefObject<HTMLElement>;
}) {
  switch (state.status) {
    case "none":
      return (
        <Centered>
          <p className="text-title3 font-regular text-label-2">No Session Selected</p>
        </Centered>
      );
    case "loading":
      return (
        <p className="sr-only" role="status">
          Opening session…
        </p>
      );
    case "missing":
      return (
        <Centered role="status">
          <p className="text-title3 font-regular text-label-2">This session is no longer in the archive.</p>
          <p className="text-callout text-label-2">It may have been forgotten. Choose another one from the list.</p>
        </Centered>
      );
    case "failed":
      return (
        <Centered role="alert">
          <p className="text-title3 font-regular text-label-2">Lore could not open this session.</p>
          <p className="text-callout text-label-2">{state.error}</p>
          <Button onClick={onRetry}>Try Again</Button>
        </Centered>
      );
    case "ready":
      return <SessionReader key={state.id} session={state.session} {...props} />;
  }
}
