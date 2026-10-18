import { useSyncExternalStore } from "react";

/**
 * ⌘⏎ (or ⌘-click on Send) in a project session's composer: its next send starts a new session
 * in the background instead of going to this conversation. The composer marks the send, then
 * sends as usual, so its attachments and mentions are read the usual way; the conversation's
 * send takes the mark.
 */
const marked = new Set<string>();

/**
 * The `/` command's "New session" chip: the composer's next send goes to a new background
 * session and takes the chip away; a click on the chip takes it back unsent.
 */
const armed = new Set<string>();
const listeners = new Set<() => void>();

export function markBackgroundSend(conversationId: string): void {
  marked.add(conversationId);
}

/** Whether the send being made was marked or the chip was on; both go either way. */
export function takeBackgroundSend(conversationId: string | null): boolean {
  if (conversationId === null) return false;
  const chip = armed.has(conversationId);
  if (chip) armBackgroundSend(conversationId, false);
  return marked.delete(conversationId) || chip;
}

/** Drops a mark whose send never came (its attachments failed, or it was cancelled). */
export function clearBackgroundSend(conversationId: string): void {
  marked.delete(conversationId);
}

/** Turns the "New session" chip on or off for a conversation's composer. */
export function armBackgroundSend(conversationId: string, on: boolean): void {
  if (armed.has(conversationId) === on) return;
  if (on) armed.add(conversationId);
  else armed.delete(conversationId);
  for (const listener of listeners) listener();
}

/** Whether a conversation's composer shows the "New session" chip. */
export function useBackgroundArmed(conversationId: string | null): boolean {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => conversationId !== null && armed.has(conversationId),
  );
}
