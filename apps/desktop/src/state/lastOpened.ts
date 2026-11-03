import { request } from "@/ipc/client";
import type { Conversation } from "@/ipc/generated";
import { useActivity } from "@/state/activity";
import { useApp } from "@/state/store";

/**
 * One unread clock for the row and the view (D4): when a conversation last finished something
 * (a turn, a request, or an answer to another session), against the time up to which the user has
 * seen it, which the daemon keeps so every window agrees.
 */
export function finishedAt(
  conversation: { finishedAtMs?: number; peerAtMs?: number } | null | undefined,
  turnEndedAtMs: number | undefined,
): number | undefined {
  const at = Math.max(turnEndedAtMs ?? 0, conversation?.finishedAtMs ?? 0, conversation?.peerAtMs ?? 0);
  return at > 0 ? at : undefined;
}

/** Whether a conversation finished something the user hasn't seen; never the one in front of them. */
export function unread(finishedAtMs: number | undefined, seenAtMs: number | undefined, open: boolean): boolean {
  return !open && finishedAtMs !== undefined && finishedAtMs > (seenAtMs ?? 0);
}

/** `id`'s finish time, from the catalog and the activity store. */
export function useFinishedAt(id: string): number | undefined {
  const finished = useApp((s) => finishedAt(s.conversations[id], undefined));
  const turnEnded = useActivity((s) => s.byConversation[id]?.turnEndedAtMs);
  return Math.max(finished ?? 0, turnEnded ?? 0) || undefined;
}

/** The user saw `id` up to `atMs` (a finish time): written only when it moves the clock on. */
export function markSeen(id: string, atMs: number): void {
  const conversation = useApp.getState().conversations[id];
  if (!conversation || (conversation.seenAtMs ?? 0) >= atMs) return;
  // At once here, so the dot clears without waiting for the daemon's event.
  useApp.setState((s) => ({ conversations: { ...s.conversations, [id]: { ...conversation, seenAtMs: atMs } } }));
  // Not written: the clock goes back, so the dot stays true and the next open tries again.
  void request({ method: "markSeen", id, atMs }).catch(() => {
    useApp.setState((s) => {
      const now = s.conversations[id];
      if (now?.seenAtMs !== atMs) return {};
      const { seenAtMs: _unsaved, ...rest } = now;
      return { conversations: { ...s.conversations, [id]: conversation.seenAtMs === undefined ? rest : { ...rest, seenAtMs: conversation.seenAtMs } } };
    });
  });
}

/** Where each window kept its own "last opened" before the daemon did. */
const LAST_OPENED_KEY = "brigadier.lastOpened";

/** Hands the old per-window times to the daemon once, so an update doesn't dot every row. */
export function migrateLastOpened(conversations: Readonly<Record<string, Conversation>>): void {
  let saved: unknown;
  try {
    saved = JSON.parse(localStorage.getItem(LAST_OPENED_KEY) ?? "null");
    localStorage.removeItem(LAST_OPENED_KEY);
  } catch {
    return;
  }
  if (!saved || typeof saved !== "object") return;
  for (const [id, at] of Object.entries(saved)) {
    if (typeof at === "number" && Number.isFinite(at) && conversations[id]) markSeen(id, at);
  }
}
