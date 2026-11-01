import { createContext, useContext } from "react";

import type { Conversation, ModelChoice } from "@/ipc/generated";
import type { Selection } from "@/state/store";

/**
 * The conversation a view shows, for the thread slots below it (which take no props). It
 * isn't always the selected one: a side chat shows beside it in the side panel.
 */
export const ViewContext = createContext<{
  selection: Selection;
  conversation: Conversation | null;
  /** The view sits inside another one's side panel (a side chat). */
  embedded: boolean;
  /** A draft's model as its composer shows it: choosing another project keeps it. */
  draftModel?: ModelChoice;
  /** When the thread's newest work block began while it works, as its header counts: the composer's "Working for" counts from it too (D94). */
  workFrom?: number | undefined;
  /** Its newest block's turn is over, but what it started still holds it open (D24): the composer still says it works. */
  workHeld?: boolean;
}>({
  selection: { type: "none" },
  conversation: null,
  embedded: false,
});

/** The conversation of the view this renders in. */
export function useViewConversation(): Conversation | null {
  return useContext(ViewContext).conversation;
}
