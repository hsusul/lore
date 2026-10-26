import { useCallback, useEffect, useRef, useState } from "react";

import { searchPage, type SearchHit } from "../ipc";

/** Search results fetched per page; "Load more" appends another page. */
export const SEARCH_PAGE = 50;
/** Coalesce rapid typing before crossing the Tauri/SQLite boundary. */
export const SEARCH_DEBOUNCE_MS = 180;

export interface UseSearchStateOptions {
  onError: (error: string) => void;
  onClearError?: () => void;
}

export function useSearchState({ onError, onClearError }: UseSearchStateOptions) {
  const [query, setQuery] = useState("");
  // `null` is the pending state; an array (including empty) is settled. Keeping
  // those mutually exclusive avoids a separate flag drifting from the results.
  const [hits, setHits] = useState<SearchHit[] | null>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);

  // Always holds the latest query so an in-flight page can tell it has been
  // superseded by newer typing and drop its (stale) results.
  const queryRef = useRef("");
  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchResultsRef = useRef<HTMLUListElement>(null);

  useEffect(() => {
    if (query.trim() === "") {
      return;
    }

    const forQuery = query;
    let cancelled = false;
    const timer = setTimeout(() => {
      void searchPage(forQuery, SEARCH_PAGE)
        .then((page) => {
          if (cancelled || queryRef.current !== forQuery) return;
          setHits(page.hits);
          setCursor(page.next_cursor);
        })
        .catch((e: unknown) => {
          if (!cancelled && queryRef.current === forQuery) {
            setHits([]);
            onError(String(e));
          }
        });
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query, onError]);

  const updateSearch = useCallback(
    (next: string) => {
      // Batch the query and its visible state so snippets from the previous query
      // cannot paint under the new text while the debounce effect is pending.
      setQuery(next);
      queryRef.current = next;
      setHits(next.trim() === "" ? [] : null);
      setCursor(null);
      onClearError?.();
    },
    [onClearError],
  );

  const loadMore = useCallback(async () => {
    if (cursor === null || loadingMore) return;
    const forQuery = queryRef.current;
    setLoadingMore(true);
    try {
      const page = await searchPage(forQuery, SEARCH_PAGE, cursor);
      if (queryRef.current !== forQuery) return; // query changed mid-flight
      setHits((prev) => [...(prev ?? []), ...page.hits]);
      setCursor(page.next_cursor);
    } catch (e) {
      onError(String(e));
    } finally {
      setLoadingMore(false);
    }
  }, [cursor, loadingMore, onError]);

  const clearSearch = useCallback(() => {
    setQuery("");
    queryRef.current = "";
    setHits([]);
    setCursor(null);
  }, []);

  return {
    query,
    hits,
    cursor,
    loadingMore,
    queryRef,
    searchInputRef,
    searchResultsRef,
    updateSearch,
    loadMore,
    clearSearch,
  };
}
