import type { Conversation } from "@/ipc/generated";
import type { Selection } from "@/state/store";

/**
 * The conversation the user had open, remembered across launches (UI audit M2): a relaunch
 * reopens it once the catalog has loaded, if it still exists and isn't archived.
 */
export const LAST_SELECTION_KEY = "brigadier.lastSelection";

/** Remembers what `selection` shows: a conversation, or nothing for the new-chat home. */
export function rememberSelection(selection: Selection): void {
  try {
    if (selection.type === "conversation") localStorage.setItem(LAST_SELECTION_KEY, selection.id);
    else if (selection.type === "draft") localStorage.removeItem(LAST_SELECTION_KEY);
    // Settings keeps what it was opened from.
  } catch {
    /* A view preference only. */
  }
}

export function savedSelection(): string | null {
  try {
    return localStorage.getItem(LAST_SELECTION_KEY);
  } catch {
    return null;
  }
}

/** The conversation to reopen: `saved`, while it exists, isn't archived and isn't a tab's side chat. */
export function reopenable(saved: string | null, conversations: Readonly<Record<string, Conversation>>): string | null {
  const conversation = saved ? conversations[saved] : undefined;
  if (!conversation || conversation.lifecycle === "archived" || conversation.tabChat) return null;
  return conversation.id;
}
