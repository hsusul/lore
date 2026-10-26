import { useEffect, useSyncExternalStore } from "react";

import { listProviders, type ProviderDto } from "../../ipc";

// Saved model providers, shared by every model picker and the providers dialog.
let providers: ProviderDto[] = [];
let requested = false;
let generation = 0;
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Reload the saved providers. Only the newest request lands, and a failure
 * (such as a backend without providers) keeps what was there.
 */
export function refreshProviders(): Promise<void> {
  requested = true;
  const mine = ++generation;
  return Promise.resolve()
    .then(() => listProviders())
    .then(
      (next) => {
        if (mine !== generation || JSON.stringify(next) === JSON.stringify(providers)) return;
        providers = next;
        listeners.forEach((listener) => listener());
      },
      () => {},
    );
}

/** The saved model providers, loaded on first use. */
export function useProviders(): ProviderDto[] {
  const current = useSyncExternalStore(subscribe, () => providers);
  useEffect(() => {
    if (!requested) void refreshProviders();
  }, []);
  return current;
}
