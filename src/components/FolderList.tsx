import { useRef, useState } from "react";

import type { FolderSummary } from "../ipc";
import { SESSION_DND_MIME } from "./SessionList";

/**
 * The user's folders in the left rail. Folders are created inline, selected to
 * filter the session list, renamed by double-click, and deleted with the ✕.
 * Dragging a thread from the session list onto a folder files it there (one
 * folder per thread).
 */
export default function FolderList({
  folders,
  selectedId,
  onSelect,
  onCreate,
  onRename,
  onDelete,
  onDropSession,
}: {
  folders: FolderSummary[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onCreate: (name: string) => void;
  onRename: (id: string, name: string) => void;
  onDelete: (id: string) => void;
  onDropSession: (folderId: string, sessionId: string) => void;
}) {
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [dragOverId, setDragOverId] = useState<string | null>(null);
  // Enter and blur both fire commit; the guard keeps that from double-creating.
  const committedRef = useRef(false);

  function openCreate() {
    committedRef.current = false;
    setNewName("");
    setCreating(true);
  }
  function commitCreate() {
    if (committedRef.current) return;
    committedRef.current = true;
    const name = newName.trim();
    setCreating(false);
    setNewName("");
    if (name) onCreate(name);
  }
  function cancelCreate() {
    committedRef.current = true;
    setCreating(false);
    setNewName("");
  }

  function startEdit(folder: FolderSummary) {
    committedRef.current = false;
    setEditName(folder.name);
    setEditingId(folder.id);
  }
  function commitEdit() {
    if (committedRef.current) return;
    committedRef.current = true;
    const name = editName.trim();
    const id = editingId;
    setEditingId(null);
    if (id && name) onRename(id, name);
  }
  function cancelEdit() {
    committedRef.current = true;
    setEditingId(null);
  }

  function hasSession(event: React.DragEvent) {
    return event.dataTransfer.types.includes(SESSION_DND_MIME);
  }

  return (
    <nav className="folders" aria-label="Folders">
      <div className="nav-heading__row">
        <h2 className="nav-heading">Folders</h2>
        <button
          type="button"
          className="nav-heading__add"
          aria-label="New folder"
          title="New folder"
          onClick={openCreate}
        >
          +
        </button>
      </div>

      {creating && (
        <input
          className="folder__field"
          autoFocus
          placeholder="Folder name"
          aria-label="New folder name"
          value={newName}
          onChange={(event) => setNewName(event.target.value)}
          onBlur={commitCreate}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitCreate();
            } else if (event.key === "Escape") {
              event.preventDefault();
              cancelCreate();
            }
          }}
        />
      )}

      {folders.length === 0 && !creating ? (
        <p className="folders__empty">
          No folders yet. Add one, then drag threads onto it.
        </p>
      ) : (
        <ul className="nav-list">
          {folders.map((folder) => (
            <li
              key={folder.id}
              className={`folder-row${dragOverId === folder.id ? " is-dragover" : ""}`}
              onDragOver={(event) => {
                if (!hasSession(event)) return;
                event.preventDefault();
                event.dataTransfer.dropEffect = "move";
                setDragOverId(folder.id);
              }}
              onDragLeave={() =>
                setDragOverId((current) => (current === folder.id ? null : current))
              }
              onDrop={(event) => {
                if (!hasSession(event)) return;
                event.preventDefault();
                setDragOverId(null);
                const sessionId = event.dataTransfer.getData(SESSION_DND_MIME);
                if (sessionId) onDropSession(folder.id, sessionId);
              }}
            >
              {editingId === folder.id ? (
                <input
                  className="folder__field"
                  autoFocus
                  aria-label={`Rename folder ${folder.name}`}
                  value={editName}
                  onChange={(event) => setEditName(event.target.value)}
                  onBlur={commitEdit}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      commitEdit();
                    } else if (event.key === "Escape") {
                      event.preventDefault();
                      cancelEdit();
                    }
                  }}
                />
              ) : (
                <>
                  <button
                    type="button"
                    className="nav-item folder__item"
                    aria-pressed={selectedId === folder.id}
                    onClick={() => onSelect(folder.id)}
                    onDoubleClick={() => startEdit(folder)}
                    onKeyDown={(event) => {
                      if (event.key === "F2") {
                        event.preventDefault();
                        startEdit(folder);
                      } else if (event.key === "Delete") {
                        event.preventDefault();
                        onDelete(folder.id);
                      }
                    }}
                    title="Open folder (F2 to rename, Delete to remove)"
                  >
                    <span className="dot dot--folder" aria-hidden />
                    <span className="nav-item__name">{folder.name}</span>
                    <span
                      className="nav-item__count"
                      aria-label={`${folder.session_count} threads`}
                    >
                      {folder.session_count}
                    </span>
                  </button>
                  <button
                    type="button"
                    className="folder__delete"
                    aria-label={`Delete folder ${folder.name}`}
                    title="Delete folder"
                    onClick={() => onDelete(folder.id)}
                  >
                    ✕
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </nav>
  );
}
