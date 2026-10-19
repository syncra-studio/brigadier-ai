/**
 * What file links in markdown need from the conversation around them. Kept apart from
 * `markdown-text`, so the views that provide or share them don't load the markdown renderer
 * at startup (it loads lazily with the first message).
 */
import { createContext, type ReactNode, useContext } from "react";

import { selectedConversation, useApp } from "@/state/store";

/** The root of the open session's checkout, for resolving file links. */
export function useCheckoutRoot(): string | null {
  return useApp((s) => {
    const setup = selectedConversation(s)?.setup;
    if (setup?.type !== "session") return null;
    return setup.environment.type === "newWorktree"
      ? (setup.environment.path ?? setup.repo)
      : setup.repo;
  });
}

/**
 * Shows a file (absolute path) at a line in the open session's Files tab; false when it isn't
 * one of the session's files.
 */
export const OpenFileContext = createContext<(path: string, line: number | null) => boolean>(
  () => false,
);

/**
 * How a `@task-N` mention in the user's own message shows (`brigadier-task:N` links): its
 * worker's chip. Without it the mention stays as written.
 */
export const TaskMentionContext = createContext<((number: number) => ReactNode) | null>(null);

/**
 * How a pasted image shows in the user's own message (`brigadier-image:n` links): its chip,
 * where it was pasted. Without it the `[Image #n]` stays as written.
 */
export const InlineImageContext = createContext<((n: number) => ReactNode) | null>(null);

/**
 * How a text pasted as a chip shows in the user's own message (`brigadier-paste:n` links): its
 * chip, where it was pasted. Without it the `[Pasted text #n]` stays as written.
 */
export const InlinePasteContext = createContext<((n: number) => ReactNode) | null>(null);

/** The conversation whose messages hold the links, where a view shows one that isn't selected. */
export const LinkConversationContext = createContext<string | null>(null);

/**
 * When the message holding the links was written: a link to `outputs/<name>` then opens the
 * copy of the task that existed by then, not a later task's file of the same name.
 */
export const LinkTimeContext = createContext<number | null>(null);

/** The conversation a file link belongs to: the view's, else the selected one. */
export function useLinkConversation(): string | null {
  const own = useContext(LinkConversationContext);
  const selected = useApp((s) => (s.selection.type === "conversation" ? s.selection.id : null));
  return own ?? selected;
}

