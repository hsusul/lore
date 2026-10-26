import { useCallback, useEffect, useRef, useState } from "react";

import {
  listDetectedAgents,
  listFolders,
  listRepositories,
  type DetectedAgent,
  type FolderSummary,
  type RepositorySummary,
} from "../ipc";

export interface Catalog {
  agents: DetectedAgent[];
  repositories: RepositorySummary[];
  folders: FolderSummary[];
}

export type CatalogState =
  | { status: "loading" }
  | { status: "failed"; error: string }
  | { status: "ready"; catalog: Catalog };

/**
 * The archive's navigational facts: agents, repositories, and folders. A failed
 * first load is `failed`, never an empty archive. A failed later refresh keeps
 * the last good catalog and reports the error to the caller instead.
 */
export function useCatalog() {
  const [state, setState] = useState<CatalogState>({ status: "loading" });
  const request = useRef(0);
  const loaded = useRef(false);

  const refresh = useCallback(async () => {
    const id = ++request.current;
    if (!loaded.current) setState({ status: "loading" });
    try {
      const [agents, repositories, folders] = await Promise.all([
        listDetectedAgents(),
        listRepositories(),
        listFolders(),
      ]);
      if (id !== request.current) return;
      loaded.current = true;
      setState({ status: "ready", catalog: { agents, repositories, folders } });
    } catch (e) {
      if (id !== request.current) return;
      if (loaded.current) throw e;
      setState({ status: "failed", error: String(e) });
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { state, refresh };
}
