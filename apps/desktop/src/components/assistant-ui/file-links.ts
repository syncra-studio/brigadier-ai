/**
 * What file links in markdown need from the conversation around them. Kept apart from
 * `markdown-text`, so the views that provide or share them don't load the markdown renderer
 * at startup (it loads lazily with the first message).
 */
import { createContext } from "react";

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
