/**
 * ⌘⏎ in a task tab's composer: its next send starts a new task in a background tab instead of
 * going to this conversation. The composer marks the send, then sends as usual, so its
 * attachments and mentions are read the usual way; the conversation's send takes the mark.
 */
const marked = new Set<string>();

export function markBackgroundSend(conversationId: string): void {
  marked.add(conversationId);
}

/** Whether the send being made was marked; the mark goes either way. */
export function takeBackgroundSend(conversationId: string | null): boolean {
  return conversationId !== null && marked.delete(conversationId);
}

/** Drops a mark whose send never came (its attachments failed, or it was cancelled). */
export function clearBackgroundSend(conversationId: string): void {
  marked.delete(conversationId);
}
