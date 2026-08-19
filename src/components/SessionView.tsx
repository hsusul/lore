import { useCallback, useState } from "react";

import { agentLabel, formatRelative, formatTime } from "../format";
import {
  listSessionMessagesPage,
  type FileEventDto,
  type GitObservationDto,
  type MessageDto,
  type MessagePartDto,
  type SessionDetail,
} from "../ipc";

const SOURCE_LABEL: Record<string, string> = {
  agent_recorded: "recorded by agent",
  agent_patch: "recorded patch",
  lore_captured: "observed by Lore",
  lore_reverified: "reverified",
};

const FILE_SOURCE_LABEL: Record<string, string> = {
  agent_patch: "recorded patch",
  agent_tool_input: "tool input",
  lore_capture: "captured",
};

const TIMELINE_PAGE = 200;
const MAX_DIFF_LINES = 1_000;

function DiffBlock({ text }: { text: string }) {
  const allLines = text.replace(/\n$/, "").split("\n");
  const truncated = allLines.length > MAX_DIFF_LINES;
  const lines = truncated ? allLines.slice(0, MAX_DIFF_LINES) : allLines;
  return (
    <div className="diff" role="group" aria-label="patch">
      {lines.map((line, index) => {
        let cls = "diff__line";
        if (line.startsWith("@@")) cls += " diff__line--hunk";
        else if (line.startsWith("+++") || line.startsWith("---")) {
          cls += " diff__line--hunk";
        } else if (line.startsWith("+")) cls += " diff__line--add";
        else if (line.startsWith("-")) cls += " diff__line--del";
        return (
          <span key={index} className={cls}>
            {line || " "}
          </span>
        );
      })}
      {truncated && (
        <span className="diff__line diff__line--hunk">
          {`… truncated (${allLines.length - MAX_DIFF_LINES} more lines)`}
        </span>
      )}
    </div>
  );
}

function Part({ part }: { part: MessagePartDto }) {
  if (part.kind === "opaque") {
    return <p className="part part--opaque">Encrypted content — not shown.</p>;
  }
  if (part.kind === "thinking") {
    return (
      <details className="part part--thinking">
        <summary>Thinking (not indexed)</summary>
        <pre>{part.text}</pre>
      </details>
    );
  }
  if (part.text != null) {
    return <p className="part">{part.text}</p>;
  }
  if (part.content_json != null) {
    return <pre className="part part--json">{`${part.kind}: ${part.content_json}`}</pre>;
  }
  return null;
}

function Message({ message }: { message: MessageDto }) {
  const textLength = message.parts.reduce(
    (total, part) => total + (part.text?.length ?? 0) + (part.content_json?.length ?? 0),
    0,
  );
  const startsOpen = textLength < 2_000;
  return (
    <details className="msg" open={startsOpen} aria-label={`message ${message.seq}`}>
      <summary className="msg__meta">
        <span className={`role-tag role-tag--${message.role}`}>{message.role}</span>
        {message.model && <span className="msg__model">{message.model}</span>}
        {!startsOpen && <span className="msg__collapsed-hint">long message · collapsed</span>}
        {message.ts != null && <time className="msg__time">{formatTime(message.ts)}</time>}
      </summary>
      <div className="msg__body">
        {message.parts.map((part) => (
          <Part key={part.ordinal} part={part} />
        ))}
      </div>
    </details>
  );
}

function GitRail({ observations }: { observations: GitObservationDto[] }) {
  if (observations.length === 0) {
    return <p className="git-rail__empty empty">No repository context.</p>;
  }
  return (
    <ul className="git-rail">
      {observations.map((observation) => (
        <li
          key={[
            observation.segment_id ?? "session",
            observation.source,
            observation.event_ts ?? "no-event",
            observation.observed_at,
            observation.commit_sha ?? "no-commit",
            observation.branch ?? "no-branch",
          ].join(":")}
          className={`git-obs git-obs--${observation.source}`}
        >
          <span className="dot" aria-hidden="true" />
          <span className="git-obs__label">
            {SOURCE_LABEL[observation.source] ?? observation.source}
          </span>
          {observation.branch && (
            <span className="git-obs__branch mono">{observation.branch}</span>
          )}
          {observation.commit_sha && (
            <code className="git-obs__commit">{observation.commit_sha.slice(0, 8)}</code>
          )}
          {observation.is_dirty === true && <span className="chip chip--warn">dirty</span>}
          {observation.commit_exists === false && (
            <span className="chip chip--danger">commit missing</span>
          )}
          <time className="git-obs__time">{formatRelative(observation.observed_at)}</time>
        </li>
      ))}
    </ul>
  );
}

function ParseNotice({
  detail,
  git,
}: {
  detail: SessionDetail;
  git: GitObservationDto[];
}) {
  const { summary, file_events, parse_note } = detail;
  if (summary.parse_status === "ok") return null;

  const failed = summary.parse_status === "failed";
  const heading = failed
    ? "This session could not be fully read"
    : "This session was only partially read";
  const diagnostic =
    parse_note ??
    (failed
      ? "Lore could not safely normalize this source format."
      : "Lore skipped one or more records it could not safely normalize.");
  const metrics = [
    ["Messages recovered", summary.message_count],
    ["Tool calls recovered", summary.tool_call_count],
    ["File changes recovered", file_events.length],
    ["Git context recovered", git.length > 0 ? "Yes" : "None"],
  ] as const;

  return (
    <section
      className={`parse-notice parse-notice--${failed ? "failed" : "partial"}`}
      aria-labelledby="parse-notice-heading"
    >
      <p className="parse-notice__eyebrow">Recovered with limits</p>
      <h3 id="parse-notice-heading">{heading}</h3>
      <p className="parse-notice__intro">
        Lore kept every normalized part it could safely recover.
      </p>
      <dl className="parse-notice__metrics">
        {metrics.map(([label, value]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd className="parse-notice__metric-value">{value}</dd>
          </div>
        ))}
      </dl>
      <p className="parse-notice__diagnostic">
        <span>Parser note</span>
        <code>{diagnostic}</code>
      </p>
      <p className="parse-notice__reassurance">
        Your original agent log is untouched. Lore can re-read it after parser support
        improves.
      </p>
    </section>
  );
}

function FileEventRow({
  fileEvent,
  loadPatch,
}: {
  fileEvent: FileEventDto;
  loadPatch?: (id: string) => Promise<string | null>;
}) {
  const [patch, setPatch] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

  async function toggle() {
    if (!open && patch === null && loadPatch) {
      setPatch((await loadPatch(fileEvent.id)) ?? "(patch unavailable)");
    }
    setOpen((v) => !v);
  }

  return (
    <li className="file">
      <div className="file__row">
        <code className="file__path">{fileEvent.path}</code>
        <span className="file__kind">{fileEvent.change_kind}</span>
        {fileEvent.lines_added != null && (
          <span className="diffstat diffstat--add">+{fileEvent.lines_added}</span>
        )}
        {fileEvent.lines_removed != null && (
          <span className="diffstat diffstat--del">−{fileEvent.lines_removed}</span>
        )}
        <span className="file-source">
          {FILE_SOURCE_LABEL[fileEvent.source] ?? fileEvent.source}
        </span>
        {fileEvent.has_patch && (
          <button type="button" className="file__toggle" onClick={toggle}>
            {open ? "Hide patch" : "View patch"}
          </button>
        )}
      </div>
      {open && patch != null && <DiffBlock text={patch} />}
    </li>
  );
}

type SessionViewProps = {
  detail: SessionDetail | null;
  git: GitObservationDto[];
  loadPatch?: (id: string) => Promise<string | null>;
  secretCount?: number;
  onExport?: () => void;
  onSaveFile?: () => void;
  onForget?: () => void;
};

type SessionContentProps = Omit<SessionViewProps, "detail"> & {
  detail: SessionDetail;
};

function SessionContent({
  detail,
  git,
  loadPatch,
  secretCount = 0,
  onExport,
  onSaveFile,
  onForget,
}: SessionContentProps) {
  const [messages, setMessages] = useState<MessageDto[]>(detail.messages);
  const [nextCursor, setNextCursor] = useState<string | null>(detail.next_message_cursor ?? null);
  const [visibleMessages, setVisibleMessages] = useState(TIMELINE_PAGE);
  const [loadingMore, setLoadingMore] = useState(false);

  const { summary, segments, file_events } = detail;

  const handleShowMore = useCallback(async () => {
    if (visibleMessages < messages.length) {
      setVisibleMessages((count) => count + TIMELINE_PAGE);
      return;
    }
    if (nextCursor && !loadingMore) {
      setLoadingMore(true);
      try {
        const page = await listSessionMessagesPage(summary.id, TIMELINE_PAGE, nextCursor);
        setMessages((prev) => [...prev, ...page.messages]);
        setNextCursor(page.next_cursor);
        setVisibleMessages((count) => count + page.messages.length);
      } finally {
        setLoadingMore(false);
      }
    }
  }, [visibleMessages, messages.length, nextCursor, loadingMore, summary.id]);

  const totalMessages = Math.max(summary.message_count, messages.length);
  const hasMore = visibleMessages < totalMessages || nextCursor !== null;

  return (
    <section className="session" aria-label="Session detail">
      <header className="session__header">
        <h2>{summary.title ?? "(untitled session)"}</h2>
        <p className="session__meta">
          <span className="chip chip--agent">{agentLabel(summary.agent_id)}</span>
          {summary.primary_model && <span className="mono">{summary.primary_model}</span>}
          <span>{totalMessages} messages</span>
          <span>{summary.tool_call_count} tools</span>
          {segments.length > 1 && <span>{segments.length} segments</span>}
          {summary.started_at != null && <span>{formatRelative(summary.started_at)}</span>}
          {summary.parse_status !== "ok" && (
            <span
              className={`chip ${summary.parse_status === "failed" ? "chip--danger" : "chip--warn"}`}
            >
              {summary.parse_status}
            </span>
          )}
          {secretCount > 0 && (
            <span
              className="chip chip--warn"
              title="Flagged secrets are redacted from search and default exports. The canonical copy may still contain them."
            >
              🔑 {secretCount} flagged
            </span>
          )}
        </p>
        {(onExport || onSaveFile || onForget) && (
          <div className="session__actions">
            {onExport && (
              <button type="button" className="btn--ghost" onClick={onExport}>
                Copy Markdown
              </button>
            )}
            {onSaveFile && (
              <button type="button" className="btn--ghost" onClick={onSaveFile}>
                Save file
              </button>
            )}
            {onForget && (
              <button type="button" className="btn--ghost session__forget" onClick={onForget}>
                Forget
              </button>
            )}
          </div>
        )}
      </header>

      <ParseNotice detail={detail} git={git} />

      <section aria-labelledby="git-heading">
        <h3 id="git-heading" className="section-title">
          Git
        </h3>
        <GitRail observations={git} />
      </section>

      {file_events.length > 0 && (
        <section aria-labelledby="files-heading">
          <h3 id="files-heading" className="section-title">
            Files
          </h3>
          <ul className="files">
            {file_events.map((fileEvent) => (
              <FileEventRow key={fileEvent.id} fileEvent={fileEvent} loadPatch={loadPatch} />
            ))}
          </ul>
        </section>
      )}

      <section aria-labelledby="timeline-heading">
        <h3 id="timeline-heading" className="section-title">
          Timeline
        </h3>
        <div className="timeline">
          {messages.slice(0, visibleMessages).map((message) => (
            <Message key={message.id} message={message} />
          ))}
        </div>
        {hasMore && (
          <button
            type="button"
            className="timeline__more btn--ghost"
            disabled={loadingMore}
            onClick={handleShowMore}
          >
            {loadingMore
              ? "Loading messages..."
              : `Show more messages (${Math.min(visibleMessages, totalMessages)} of ${totalMessages})`}
          </button>
        )}
      </section>
    </section>
  );
}

export default function SessionView({ detail, ...props }: SessionViewProps) {
  if (!detail) {
    return (
      <section className="session session--empty" aria-label="Session detail">
        <p>Select a session to read it.</p>
      </section>
    );
  }

  return <SessionContent key={detail.summary.id} detail={detail} {...props} />;
}
