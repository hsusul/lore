import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { parseUnifiedDiff, type DiffFile } from "../../diff";
import { taskDiff, type TaskDiffDto } from "../../ipc";
import { RefreshIcon } from "./icons";
import { errorText } from "./state";

const STATUS_LETTER: Record<DiffFile["status"], string> = {
  added: "A",
  deleted: "D",
  modified: "M",
  renamed: "R",
};

type Props = {
  taskId: string;
  title: string;
  /** Inside the agent view: no page title, since the agent header already names the task. */
  embedded?: boolean;
  /** Changes when the task's files or commits move, to reload without a click. */
  refreshKey?: string;
};

/** A task's worktree diff against its base commit. */
export default function DiffView({ taskId, title, embedded = false, refreshKey }: Props) {
  const [diff, setDiff] = useState<TaskDiffDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const fileRefs = useRef<(HTMLElement | null)[]>([]);
  const alive = useRef(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const next = await taskDiff(taskId);
      if (!alive.current) return;
      setDiff(next);
      setError(null);
    } catch (e) {
      if (alive.current) setError(errorText(e));
    } finally {
      if (alive.current) setLoading(false);
    }
  }, [taskId]);

  useEffect(() => {
    alive.current = true;
    void load();
    return () => {
      alive.current = false;
    };
  }, [load, refreshKey]);

  const files = useMemo(() => (diff ? parseUnifiedDiff(diff.text) : []), [diff]);
  const additions = files.reduce((n, f) => n + f.additions, 0);
  const deletions = files.reduce((n, f) => n + f.deletions, 0);

  return (
    <div className={`diff-view${embedded ? " diff-view--embedded" : ""}`}>
      <header className="diff-view__header">
        <h2 className={embedded ? "visually-hidden" : "diff-view__title"}>Changes in {title}</h2>
        {diff && (
          <span className="diff-view__stats">
            {files.length} {files.length === 1 ? "file" : "files"}
            <span className="diff-add-text"> +{additions}</span>
            <span className="diff-del-text"> −{deletions}</span>
          </span>
        )}
        <span className="agent-view__spacer" />
        <button type="button" className="wb-btn wb-btn--small" disabled={loading} onClick={() => void load()}>
          <RefreshIcon /> Refresh
        </button>
      </header>
      <div className="diff-view__body">
        {error && (
          <p className="wb-note wb-note--error" role="alert">
            {error}
          </p>
        )}
        {diff?.truncated && (
          <p className="wb-notice" role="note">
            The diff is too large and was truncated.
          </p>
        )}
        {!diff && !error && (
          <p className="wb-note" role="status">
            Loading diff…
          </p>
        )}
        {diff && files.length === 0 && <p className="wb-note">No changes.</p>}
        {files.length > 1 && (
          <ul className="diff-view__files" aria-label="Changed files">
            {files.map((file, i) => (
              <li key={`${file.path}-${i}`}>
                <button
                  type="button"
                  className="diff-view__file-link"
                  onClick={() => fileRefs.current[i]?.scrollIntoView?.({ block: "start" })}
                >
                  <span className={`diff-status diff-status--${file.status}`}>{STATUS_LETTER[file.status]}</span>
                  <span className="mono">{file.path}</span>
                  <span className="diff-add-text">+{file.additions}</span>
                  <span className="diff-del-text">−{file.deletions}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
        {files.map((file, i) => (
          <section
            key={`${file.path}-${i}`}
            ref={(el) => {
              fileRefs.current[i] = el;
            }}
            className="diff-file"
            aria-label={file.path}
          >
            <h3 className="diff-file__header">
              <span className={`diff-status diff-status--${file.status}`} title={file.status}>
                {STATUS_LETTER[file.status]}
              </span>
              <span className="mono">
                {file.status === "renamed" && file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
              </span>
            </h3>
            {file.binary ? (
              <p className="wb-note">Binary file not shown.</p>
            ) : file.hunks.length === 0 ? (
              <p className="wb-note">No content changes.</p>
            ) : (
              <div className="diff-file__scroll">
                <table className="diff-table">
                  {file.hunks.map((hunk, h) => (
                    <tbody key={h}>
                      <tr className="diff-line diff-line--hunk">
                        <td className="diff-line__no" />
                        <td className="diff-line__no" />
                        <td className="diff-line__text">{hunk.header}</td>
                      </tr>
                      {hunk.lines.map((line, l) => (
                        <tr key={l} className={`diff-line diff-line--${line.type}`}>
                          <td className="diff-line__no">{line.oldNo ?? ""}</td>
                          <td className="diff-line__no">{line.newNo ?? ""}</td>
                          <td className="diff-line__text">
                            <span className="diff-line__sign" aria-hidden="true">
                              {line.type === "add" ? "+" : line.type === "del" ? "-" : " "}
                            </span>
                            {line.text}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  ))}
                </table>
              </div>
            )}
          </section>
        ))}
      </div>
    </div>
  );
}
