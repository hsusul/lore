import { useCallback, useEffect, useRef, useState } from "react";

export interface Page<T> {
  items: T[];
  cursor: string | null;
}

/** Fetch one page. `limit` may exceed the page size when a refresh keeps depth. */
export type FetchPage<T> = (limit: number, cursor: string | null) => Promise<Page<T>>;

export type PagedState<T> =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "failed"; error: string }
  | {
      status: "ready";
      items: T[];
      cursor: string | null;
      more: "idle" | "loading" | "failed";
    };

/**
 * A keyset-paged list. A new `fetchPage` identity starts over from the first
 * page; `null` means there is nothing to fetch. Every request carries a
 * generation, so a response that lost a race to a newer request is dropped.
 * `refresh` reloads in place at the current depth without flashing a loading
 * state, which keeps a background scan from collapsing a scrolled list.
 */
export function usePagedList<T>(
  fetchPage: FetchPage<T> | null,
  pageSize: number,
  keyOf: (item: T) => string,
) {
  const [state, setState] = useState<PagedState<T>>(
    fetchPage ? { status: "loading" } : { status: "idle" },
  );
  const generation = useRef(0);
  const stateRef = useRef(state);
  stateRef.current = state;

  useEffect(() => {
    const gen = ++generation.current;
    if (!fetchPage) {
      setState({ status: "idle" });
      return;
    }
    setState({ status: "loading" });
    fetchPage(pageSize, null).then(
      (page) => {
        if (gen !== generation.current) return;
        setState({ status: "ready", items: page.items, cursor: page.cursor, more: "idle" });
      },
      (e: unknown) => {
        if (gen !== generation.current) return;
        setState({ status: "failed", error: String(e) });
      },
    );
  }, [fetchPage, pageSize]);

  const loadMore = useCallback(async () => {
    const current = stateRef.current;
    if (!fetchPage || current.status !== "ready") return;
    if (current.cursor === null || current.more === "loading") return;
    const gen = ++generation.current;
    setState({ ...current, more: "loading" });
    try {
      const page = await fetchPage(pageSize, current.cursor);
      if (gen !== generation.current) return;
      // Pages are disjoint by contract; deduping keeps a concurrent archive
      // refresh from ever painting the same row twice.
      const seen = new Set(current.items.map(keyOf));
      setState({
        status: "ready",
        items: [...current.items, ...page.items.filter((item) => !seen.has(keyOf(item)))],
        cursor: page.cursor,
        more: "idle",
      });
    } catch {
      if (gen !== generation.current) return;
      setState({ ...current, more: "failed" });
    }
  }, [fetchPage, keyOf, pageSize]);

  const refresh = useCallback(async () => {
    if (!fetchPage) return;
    const current = stateRef.current;
    const depth = current.status === "ready" ? current.items.length : 0;
    const gen = ++generation.current;
    if (current.status !== "ready") setState({ status: "loading" });
    try {
      const page = await fetchPage(Math.max(pageSize, depth), null);
      if (gen !== generation.current) return;
      setState({ status: "ready", items: page.items, cursor: page.cursor, more: "idle" });
    } catch (e) {
      if (gen !== generation.current) return;
      // A failed background refresh keeps the rows the user is looking at.
      if (stateRef.current.status !== "ready") setState({ status: "failed", error: String(e) });
      else throw e;
    }
  }, [fetchPage, pageSize]);

  return { state, loadMore, refresh };
}
