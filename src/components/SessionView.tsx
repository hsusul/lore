import { useState } from "react";

import type {
  FileEventDto,
  GitObservationDto,
  MessageDto,
  MessagePartDto,
  SessionDetail,
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

function formatTime(ms: number | null): string {
  if (ms == null) return "";
  return new Date(ms).toLocaleString();
}

function Part({ part }: { part: MessagePartDto }) {
  if (part.kind === "opaque") {
    return (
      <p className="part part--opaque">
        <em>Encrypted content — not shown.</em>
      </p>
    );
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
    return (
      <pre className="part part--json">
        {`${part.kind}: ${part.content_json}`}
      </pre>
    );
  }
  return null;
}

function Message({ message }: { message: MessageDto }) {
  return (
    <article className={`msg msg--${message.role}`} aria-label={`message ${message.seq}`}>
      <header className="msg__meta">
        <span className="msg__role">{message.role}</span>
        {message.model && <span className="msg__model">{message.model}</span>}
        {message.ts != null && (
          <time className="msg__ts">{formatTime(message.ts)}</time>
        )}
      </header>
      {message.parts.map((part) => (
        <Part key={part.ordinal} part={part} />
      ))}
    </article>
  );
}

function GitRail({ observations }: { observations: GitObservationDto[] }) {
  if (observations.length === 0) {
    return <p className="git-rail__empty">No repository context.</p>;
  }
  return (
    <ul className="git-rail">
      {observations.map((observation, index) => (
        <li key={`${observation.source}-${index}`} className={`git-obs git-obs--${observation.source}`}>
          <span className="git-obs__label">
            {SOURCE_LABEL[observation.source] ?? observation.source}
          </span>
          {observation.branch && <span className="git-obs__branch">{observation.branch}</span>}
          {observation.commit_sha && (
            <code className="git-obs__commit">{observation.commit_sha.slice(0, 8)}</code>
          )}
          {observation.is_dirty === true && <span className="git-obs__dirty">dirty</span>}
          {observation.commit_exists === false && (
            <span className="git-obs__missing">commit missing</span>
          )}
          <time className="git-obs__time">{formatTime(observation.observed_at)}</time>
        </li>
      ))}
    </ul>
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
    <li>
      <code>{fileEvent.path}</code> · {fileEvent.change_kind}
      {fileEvent.lines_added != null && (
        <span className="diffstat"> +{fileEvent.lines_added}</span>
      )}
      {fileEvent.lines_removed != null && (
        <span className="diffstat"> −{fileEvent.lines_removed}</span>
      )}
      <span className="file-source">
        {FILE_SOURCE_LABEL[fileEvent.source] ?? fileEvent.source}
      </span>
      {fileEvent.has_patch && (
        <>
          <button className="file-patch__toggle" onClick={toggle}>
            {open ? "Hide patch" : "View patch"}
          </button>
          {open && <pre className="file-patch">{patch}</pre>}
        </>
      )}
    </li>
  );
}

export default function SessionView({
  detail,
  git,
  loadPatch,
}: {
  detail: SessionDetail | null;
  git: GitObservationDto[];
  loadPatch?: (id: string) => Promise<string | null>;
}) {
  if (!detail) {
    return (
      <section className="session session--empty" aria-label="session">
        <p>Select a session to read it.</p>
      </section>
    );
  }

  const { summary, messages, file_events } = detail;
  return (
    <section className="session" aria-label="session">
      <header className="session__header">
        <h2>{summary.title ?? "(untitled session)"}</h2>
        <p className="session__meta">
          {summary.agent_id}
          {summary.primary_model ? ` · ${summary.primary_model}` : ""} ·{" "}
          {summary.message_count} messages
          {summary.parse_status !== "ok" && (
            <span className={`badge badge--${summary.parse_status}`}>
              {summary.parse_status}
            </span>
          )}
        </p>
      </header>

      <section aria-labelledby="git-heading" className="session__git">
        <h3 id="git-heading">Git</h3>
        <GitRail observations={git} />
      </section>

      {file_events.length > 0 && (
        <section aria-labelledby="files-heading" className="session__files">
          <h3 id="files-heading">Files</h3>
          <ul>
            {file_events.map((fileEvent, index) => (
              <FileEventRow
                key={`${fileEvent.id}-${index}`}
                fileEvent={fileEvent}
                loadPatch={loadPatch}
              />
            ))}
          </ul>
        </section>
      )}

      <section aria-labelledby="timeline-heading" className="session__timeline">
        <h3 id="timeline-heading">Timeline</h3>
        {messages.map((message) => (
          <Message key={message.id} message={message} />
        ))}
      </section>
    </section>
  );
}
