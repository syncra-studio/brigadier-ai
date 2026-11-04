import { useMemo } from "react";

import { forksUnderSources } from "@/app/conversation/forks";
import type { Conversation } from "@/ipc/generated";
import { useApp } from "@/state/store";

/** The inbox's sections, top to bottom; Archived docks above the footer. */
export const INBOX_SECTIONS = ["pinned", "open", "archived"] as const;
export type InboxSection = (typeof INBOX_SECTIONS)[number];
export type Inbox = Record<InboxSection, Conversation[]>;

/**
 * A row's place in Open: when the user last sent it a message, else when it was created. Nothing a
 * turn does, nor sleep, wake or a rename, changes it, so a row stays put while it works.
 */
export function sortTime(conversation: Conversation): number {
  return conversation.lastSentAtMs ?? conversation.createdAtMs;
}

/** Every listed row in its section and order; Open includes forks in last-send order. */
export function inboxLists(conversations: readonly Conversation[]): Inbox {
  const listed = conversations.filter((conversation) => !conversation.sideOf);
  const archived = listed.filter((conversation) => conversation.lifecycle === "archived");
  const live = listed.filter((conversation) => conversation.lifecycle !== "archived");
  return {
    pinned: forksUnderSources(
      live
        .filter((conversation) => conversation.pinnedAtMs !== null)
        .toSorted((a, b) => (b.pinnedAtMs ?? 0) - (a.pinnedAtMs ?? 0)),
    ),
    open: live
      .filter((conversation) => conversation.pinnedAtMs === null)
      .toSorted((a, b) => sortTime(b) - sortTime(a) || b.id.localeCompare(a.id)),
    // A late finish or rename must not reorder archived rows either.
    archived: archived.toSorted((a, b) => sortTime(b) - sortTime(a) || b.id.localeCompare(a.id)),
  };
}

/** The inbox as the sidebar shows it. */
export function useInbox(): Inbox {
  const conversations = useApp((s) => s.conversations);
  return useMemo(() => inboxLists(Object.values(conversations)), [conversations]);
}
