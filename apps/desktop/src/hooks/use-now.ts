import { useEffect, useState, useSyncExternalStore } from "react";

/** The time now, read again every `everyMs` (never when null). */
export function useNow(everyMs: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (everyMs === null) return;
    const timer = window.setInterval(() => setNow(Date.now()), everyMs);
    return () => window.clearInterval(timer);
  }, [everyMs]);
  return now;
}

const SHARED_EVERY_MS = 30_000;
let sharedNow = Date.now();
let sharedTimer: number | null = null;
const sharedListeners = new Set<() => void>();

function subscribeShared(listener: () => void): () => void {
  sharedListeners.add(listener);
  if (sharedTimer === null) {
    // Nothing kept the time while nobody listened.
    sharedNow = Date.now();
    sharedTimer = window.setInterval(() => {
      sharedNow = Date.now();
      for (const each of sharedListeners) each();
    }, SHARED_EVERY_MS);
  }
  return () => {
    sharedListeners.delete(listener);
    if (sharedListeners.size === 0 && sharedTimer !== null) {
      window.clearInterval(sharedTimer);
      sharedTimer = null;
    }
  };
}

/** The time now from one clock all its users share, read every half minute: for long lists of times. */
export function useSharedNow(): number {
  return useSyncExternalStore(subscribeShared, () => sharedNow);
}
