import { useEffect, useMemo, useState } from "react";

import { readWorkspaceFile, type FileContentDto } from "../../ipc";
import { highlight } from "./highlight";
import { baseName, errorText } from "./state";

/** Read-only code view of one workspace file with a line-number gutter. */
export default function FileView({ root, relPath }: { root: string; relPath: string }) {
  const [file, setFile] = useState<FileContentDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    readWorkspaceFile(root, relPath)
      .then((content) => {
        if (!cancelled) setFile(content);
      })
      .catch((e) => {
        if (!cancelled) setError(errorText(e));
      });
    return () => {
      cancelled = true;
    };
  }, [root, relPath]);

  const text = file?.text ?? null;
  const lineCount = useMemo(() => {
    if (text === null) return 0;
    const n = text.split("\n").length;
    return text.endsWith("\n") ? n - 1 : n;
  }, [text]);
  const gutter = useMemo(
    () => Array.from({ length: Math.max(lineCount, 1) }, (_, i) => i + 1).join("\n"),
    [lineCount],
  );
  const body = useMemo(() => (text === null ? null : highlight(text, baseName(relPath))), [text, relPath]);

  return (
    <div className="file-view">
      <nav className="file-view__crumbs mono" aria-label="File path">
        {relPath.split("/").map((part, i, all) => (
          <span key={i} className="file-view__crumb">
            {part}
            {i < all.length - 1 && <span className="file-view__sep" aria-hidden="true">›</span>}
          </span>
        ))}
      </nav>
      {error ? (
        <p className="wb-note wb-note--error wb-view-pad" role="alert">
          {error}
        </p>
      ) : !file ? (
        <p className="wb-note wb-view-pad" role="status">
          Loading…
        </p>
      ) : text === null ? (
        <p className="wb-note wb-view-pad">Binary file not shown.</p>
      ) : (
        <>
          {file.truncated && (
            <p className="wb-notice" role="note">
              Showing first 1 MB of this file.
            </p>
          )}
          <div className="code" tabIndex={0} aria-label={`Contents of ${relPath}`}>
            <pre className="code__gutter" aria-hidden="true">
              {gutter}
            </pre>
            <pre className="code__text">
              <code>{body}</code>
            </pre>
          </div>
        </>
      )}
    </div>
  );
}
