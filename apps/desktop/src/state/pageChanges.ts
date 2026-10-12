/** A page changed on disk: saved from any window, edited by another program, or deleted
 * (`revision` null). An editor showing an older revision reads the page again. */
export type PageChanged = { id: string; revision: string | null };

const listeners = new Set<(change: PageChanged) => void>();

/** Hears every page change from now on. Returns the unsubscribe. */
export function onPageChanged(listener: (change: PageChanged) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Passes a change from the bridge to every listener. */
export function emitPageChanged(change: PageChanged): void {
  for (const listener of listeners) listener(change);
}
