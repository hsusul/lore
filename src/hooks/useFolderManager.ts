import { useCallback, useRef, useState } from "react";

import {
  createFolder,
  deleteFolder,
  listFolders,
  renameFolder,
  setSessionFolder,
  type FolderSummary,
} from "../ipc";

export interface UseFolderManagerOptions {
  onError: (error: string) => void;
  onNotice: (notice: string) => void;
  onClearError?: () => void;
  onReloadSessions: (repo: string | null, folder: string | null) => Promise<void>;
  onFallBackToAllSessions: () => Promise<void>;
}

export function useFolderManager({
  onError,
  onNotice,
  onClearError,
  onReloadSessions,
  onFallBackToAllSessions,
}: UseFolderManagerOptions) {
  const [folders, setFolders] = useState<FolderSummary[]>([]);
  const [selectedFolder, setSelectedFolder] = useState<string | null>(null);
  const selectedFolderRef = useRef<string | null>(null);

  const refreshFolders = useCallback(async () => {
    try {
      setFolders(await listFolders());
    } catch (e) {
      onError(String(e));
    }
  }, [onError]);

  const selectFolder = useCallback(
    async (folderId: string) => {
      selectedFolderRef.current = folderId;
      setSelectedFolder(folderId);
      onClearError?.();
      try {
        await onReloadSessions(null, folderId);
      } catch (e) {
        onError(String(e));
      }
    },
    [onClearError, onError, onReloadSessions],
  );

  const handleCreateFolder = useCallback(
    async (name: string) => {
      onClearError?.();
      try {
        await createFolder(name);
        await refreshFolders();
      } catch (e) {
        onError(String(e));
      }
    },
    [onClearError, onError, refreshFolders],
  );

  const handleRenameFolder = useCallback(
    async (id: string, name: string) => {
      try {
        await renameFolder(id, name);
        await refreshFolders();
      } catch (e) {
        onError(String(e));
      }
    },
    [onError, refreshFolders],
  );

  const handleDeleteFolder = useCallback(
    async (id: string) => {
      if (!window.confirm("Delete this folder? Its threads stay in Lore, just unfiled.")) {
        return;
      }
      try {
        await deleteFolder(id);
        // Leaving a now-deleted folder view falls back to All sessions.
        if (selectedFolderRef.current === id) {
          await onFallBackToAllSessions();
        }
        await refreshFolders();
      } catch (e) {
        onError(String(e));
      }
    },
    [onError, onFallBackToAllSessions, refreshFolders],
  );

  // File a thread into a folder (or unfile it when `folderId` is null), then
  // refresh folder counts and the current pane so membership changes show at once.
  const fileSession = useCallback(
    async (
      sessionId: string,
      folderId: string | null,
      currentRepo: string | null,
    ) => {
      onClearError?.();
      try {
        await setSessionFolder(sessionId, folderId);
        await refreshFolders();
        await onReloadSessions(currentRepo, selectedFolderRef.current);
        const target = folders.find((f) => f.id === folderId);
        onNotice(
          folderId
            ? `Filed thread in “${target?.name ?? "folder"}”.`
            : "Removed thread from its folder.",
        );
      } catch (e) {
        onError(String(e));
      }
    },
    [folders, onClearError, onError, onNotice, onReloadSessions, refreshFolders],
  );

  const clearFolderSelection = useCallback(() => {
    selectedFolderRef.current = null;
    setSelectedFolder(null);
  }, []);

  return {
    folders,
    setFolders,
    selectedFolder,
    setSelectedFolder,
    selectedFolderRef,
    refreshFolders,
    selectFolder,
    handleCreateFolder,
    handleRenameFolder,
    handleDeleteFolder,
    fileSession,
    clearFolderSelection,
  };
}
