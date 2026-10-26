import { useState } from "react";

import { getFilePatch, type FileEventDto } from "../ipc";
import Diff from "./Diff";

const SOURCE: Record<string, string> = {
  agent_patch: "recorded patch",
  agent_tool_input: "from tool input",
  lore_capture: "captured by Lore",
};

type PatchState =
  | { status: "closed" }
  | { status: "loading" }
  | { status: "open"; text: string | null }
  | { status: "failed"; error: string };

function editorHref(path: string): string {
  return `vscode://file/${encodeURI(path.startsWith("/") ? path : `/${path}`)}`;
}

function FileRow({ file }: { file: FileEventDto }) {
  const [patch, setPatch] = useState<PatchState>({ status: "closed" });

  async function toggle() {
    if (patch.status === "open" || patch.status === "failed") {
      setPatch({ status: "closed" });
      return;
    }
    setPatch({ status: "loading" });
    try {
      setPatch({ status: "open", text: await getFilePatch(file.id) });
    } catch (e) {
      setPatch({ status: "failed", error: String(e) });
    }
  }

  const expanded = patch.status === "open" || patch.status === "failed";
  return (
    <li className="file">
      <div className="file__row">
        <span className={`file__kind file__kind--${file.change_kind}`}>{file.change_kind}</span>
        <code className="file__path" title={file.old_path ? `${file.old_path} → ${file.path}` : file.path}>
          {file.path}
        </code>
        <span className="file__stat">
          {file.lines_added != null && <span className="stat--add">+{file.lines_added}</span>}
          {file.lines_removed != null && <span className="stat--del">−{file.lines_removed}</span>}
        </span>
        <span className="file__source">{SOURCE[file.source] ?? file.source}</span>
        <span className="file__actions">
          {file.has_patch && (
            <button
              type="button"
              className="btn btn--quiet"
              aria-expanded={expanded}
              disabled={patch.status === "loading"}
              onClick={() => void toggle()}
            >
              {patch.status === "loading" ? "Loading…" : expanded ? "Hide patch" : "Show patch"}
            </button>
          )}
          <a
            className="btn btn--quiet"
            href={editorHref(file.path)}
            target="_blank"
            rel="noreferrer"
            title={`Open ${file.path} in your editor`}
          >
            Open
          </a>
        </span>
      </div>
      {patch.status === "open" &&
        (patch.text ? (
          <Diff text={patch.text} label={`Patch for ${file.path}`} />
        ) : (
          <p className="quiet">The patch for this change was not stored.</p>
        ))}
      {patch.status === "failed" && (
        <p className="quiet" role="alert">
          Could not load the patch: {patch.error}
        </p>
      )}
    </li>
  );
}

export default function FileChanges({ files }: { files: FileEventDto[] }) {
  if (files.length === 0) {
    return <p className="quiet">This session did not touch any files Lore could see.</p>;
  }
  return (
    <ul className="files">
      {files.map((file) => (
        <FileRow key={file.id} file={file} />
      ))}
    </ul>
  );
}
