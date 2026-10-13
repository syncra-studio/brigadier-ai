import { create } from "zustand";

/**
 * When the user last had each conversation open, remembered with the app's other view state: a
 * session that received a message from another session since shows an unread dot in the
 * sidebar (SESSION-MESSAGING-PLAN.md §6.4).
 */
export const LAST_OPENED_KEY = "brigadier.lastOpened";

function savedLastOpened(): Record<string, number> {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(LAST_OPENED_KEY) ?? "{}");
    if (!value || typeof value !== "object") return {};
    return Object.fromEntries(
      Object.entries(value).filter(([, at]) => typeof at === "number" && Number.isFinite(at)),
    );
  } catch {
    return {};
  }
}

export const useLastOpened = create<{ at: Record<string, number> }>(() => ({ at: savedLastOpened() }));

/** The user has `id` open at `atMs` (or saw what came until then). */
export function markOpened(id: string, atMs: number): void {
  const { at } = useLastOpened.getState();
  if ((at[id] ?? 0) >= atMs) return;
  const next = { ...at, [id]: atMs };
  useLastOpened.setState({ at: next });
  try {
    localStorage.setItem(LAST_OPENED_KEY, JSON.stringify(next));
  } catch {
    /* Keep in memory. */
  }
}

/** Whether a session got a message from another session since the user last had it open. */
export function unread(peerAtMs: number | undefined, lastOpenedMs: number | undefined, open: boolean): boolean {
  return !open && peerAtMs !== undefined && peerAtMs > (lastOpenedMs ?? 0);
}
