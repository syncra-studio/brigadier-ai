import { createContext } from "react";

import { latestRequest } from "@/app/conversation/rework";
import { useBoard } from "@/state/board";

/**
 * The guess at the user's next message (`UserRequest.suggestion`): the one the conversation's
 * final answer ended with (a hidden `[next: …]` line), at no request of its own. Shown
 * in the empty composer in the placeholder's colour once the request it follows is over. Tab,
 * → or Enter puts it in the field,
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
