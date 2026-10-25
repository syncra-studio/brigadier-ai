import { create } from "zustand";

import { keepWhole, type SentDraft } from "@/app/conversation/composerDraft";
import { loadDraft, saveDraft } from "@/state/drafts";

/**
 * Direct messages a worker didn't take, each kept whole with the worker it was for, apart from
 * the composer's own draft, when it couldn't go back into it as it was (the user left, picked
 * another To:, or typed meanwhile). Each is a draft of its own, so its attachments stay pinned,
 * after a restart too; its composer offers it back until it is restored or discarded.
 */
export type HeldDirect = { scope: string; conversationId: string; taskId: string; atMs: number };

const INDEX_KEY = "brigadier.directHeld";

function readIndex(): HeldDirect[] {
  try {
    const stored = localStorage.getItem(INDEX_KEY);
    return stored ? (JSON.parse(stored) as HeldDirect[]) : [];
  } catch {
    return [];
  }
}

function writeIndex(held: HeldDirect[]): void {
  try {
    localStorage.setItem(INDEX_KEY, JSON.stringify(held));
  } catch {
    // Storage can be full or unavailable; the message is then held until the app closes.
  }
  useHeldDirect.setState({ held });
}

export const useHeldDirect = create<{ held: HeldDirect[] | null }>(() => ({ held: null }));

function heldNow(): HeldDirect[] {
  return useHeldDirect.getState().held ?? readIndex();
}

/** Keeps `draft`, for worker `taskId` of `conversationId`, whole and apart. */
export function holdDirect(conversationId: string, taskId: string, draft: SentDraft): void {
  // A draft scope the daemon pins by: letters, digits and dashes.
  const scope = `held-${crypto.randomUUID()}`;
  keepWhole(scope, draft);
  writeIndex([...heldNow(), { scope, conversationId, taskId, atMs: Date.now() }]);
}

/** A held message's words, attachments and mentions; null once it's gone. */
export function heldDraft(scope: string): SentDraft | null {
  const draft = loadDraft(scope);
  if (!draft) return null;
  return { text: draft.text, attachments: draft.attachments, mentions: (draft.mentions ?? []).map((entry) => entry.mention) };
}

/** Lets a held message go (restored or discarded), and its attachments' pins with it. */
export function dropHeldDirect(scope: string): void {
  saveDraft(scope, "", [], []);
  writeIndex(heldNow().filter((entry) => entry.scope !== scope));
}

/** The direct messages held for a conversation, oldest first. */
export function useHeldDirectOf(conversationId: string | null): HeldDirect[] {
  const held = useHeldDirect((s) => s.held);
  if (held === null) {
    // Read once, the first time any composer asks.
    queueMicrotask(() => {
      if (useHeldDirect.getState().held === null) useHeldDirect.setState({ held: readIndex() });
    });
    return [];
  }
  return held.filter((entry) => entry.conversationId === conversationId);
}
