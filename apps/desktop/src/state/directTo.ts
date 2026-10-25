import { useSyncExternalStore } from "react";

import { armBackgroundSend } from "@/state/backgroundSend";

/**
 * The composer's To: one of the session's workers its next message goes to directly, once,
 * instead of to the session (the `@` menu's "Message directly"). The send takes it away once
 * the worker took the message; a click on its chip, or leaving the session, takes it back.
 */
const recipients = new Map<string, string>();
/** Conversations whose direct message is on its way: none goes twice. */
const sending = new Set<string>();
const listeners = new Set<() => void>();

function changed(): void {
  for (const listener of listeners) listener();
}

/** Sets (or, with null, takes away) the worker a conversation's next message goes to. */
export function setDirectTo(conversationId: string, taskId: string | null): void {
  if ((recipients.get(conversationId) ?? null) === taskId) return;
  if (taskId === null) recipients.delete(conversationId);
  else {
    recipients.set(conversationId, taskId);
    // One place for the next message: the worker, not a new session too.
    armBackgroundSend(conversationId, false);
  }
  changed();
}

/** The worker a conversation's next message goes to, if any. */
export function directToOf(conversationId: string | null): string | null {
  return conversationId === null ? null : (recipients.get(conversationId) ?? null);
}

/** Starts a conversation's direct send; false while one is already on its way. */
export function beginDirectSend(conversationId: string): boolean {
  if (sending.has(conversationId)) return false;
  sending.add(conversationId);
  changed();
  return true;
}

/** Whether a conversation's direct message is on its way, read when a key is pressed. */
export function directSendingOf(conversationId: string | null): boolean {
  return conversationId !== null && sending.has(conversationId);
}

export function endDirectSend(conversationId: string): void {
  if (sending.delete(conversationId)) changed();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The worker a conversation's composer shows on its To: chip. */
export function useDirectTo(conversationId: string | null): string | null {
  return useSyncExternalStore(subscribe, () => directToOf(conversationId));
}

/** Whether a conversation's direct message is on its way. */
export function useDirectSending(conversationId: string | null): boolean {
  return useSyncExternalStore(subscribe, () => conversationId !== null && sending.has(conversationId));
}
