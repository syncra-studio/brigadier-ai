import { createContext, useSyncExternalStore } from "react";

import { latestRequest } from "@/app/conversation/rework";
import { useBoard } from "@/state/board";

/**
 * The guess at the user's next message (`UserRequest.suggestion`): the one the conversation's
 * final answer ended with (a hidden `[next: …]` line), at no request of its own. Shown
 * in the empty composer in the placeholder's colour once the request it follows is over, with
 * "Press Tab to accept" until the user first takes one. Tab, → or Enter puts it in the field,
 * unsent; typing hides it until the field is empty again; sending ends it (the next request has
 * its own); an attachment hides it.
 */

/** The guess the composer shows now (none in plan mode or with an attachment), for Tab. */
export const SuggestionContext = createContext<string | null>(null);

export type Suggestion = { requestId: string; text: string };

/** The guess to show in `conversationId`'s empty composer, if any. */
export function useSuggestion(conversationId: string | null): Suggestion | null {
  const latest = useBoard((s) =>
    s.board && conversationId !== null && s.board.conversationId === conversationId ? latestRequest(s.board) : null,
  );
  const text = latest?.suggestion?.trim();
  if (!latest || !text) return null;
  // While the request works, the next message isn't due yet.
  if (latest.state.type === "working" || latest.state.type === "waiting") return null;
  return { requestId: latest.id, text };
}

const ACCEPTED_KEY = "brigadier.suggestionAccepted";
const acceptedListeners = new Set<() => void>();

function readAccepted(): boolean {
  try {
    return localStorage.getItem(ACCEPTED_KEY) === "1";
  } catch {
    return false;
  }
}

/** Whether the user has taken a guess before: "Press Tab to accept" shows only until then. */
export function useSuggestionAccepted(): boolean {
  return useSyncExternalStore((listener) => {
    acceptedListeners.add(listener);
    return () => acceptedListeners.delete(listener);
  }, readAccepted);
}

/** The user took a guess: its key hint goes for good. */
export function markSuggestionAccepted(): void {
  if (readAccepted()) return;
  try {
    localStorage.setItem(ACCEPTED_KEY, "1");
  } catch {
    // Without storage the hint stays.
  }
  for (const listener of acceptedListeners) listener();
}
