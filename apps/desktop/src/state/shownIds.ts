/**
 * The local id each confirmed message was shown under while it was pending, by its stored id.
 * The thread keeps showing it under that id: were its bubble and block mounted afresh when the
 * daemon confirms it, the thread would be shorter for a moment, its scroll position clamped,
 * and it would stop following the answer as if the user had scrolled up.
 */
const shownAs = new Map<string, string>();
const storedAs = new Map<string, string>();

/** Records that the stored message `messageId` was shown as the pending `localId`. */
export function confirmPending(messageId: string, localId: string): void {
  shownAs.set(messageId, localId);
  storedAs.set(localId, messageId);
}

/** The id a stored message is shown under in the thread. */
export function shownIdOf(messageId: string): string {
  return shownAs.get(messageId) ?? messageId;
}

/** The stored message a thread message id stands for (see `shownIdOf`). */
export function storedIdOf(id: string): string {
  return storedAs.get(id) ?? id;
}
