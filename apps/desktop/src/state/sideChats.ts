import type { Conversation } from "@/ipc/generated";

/**
 * Only a loaded session's unreferenced side chats belong to tab cleanup. Tabs' own chats are
 * the daemon's to sort out (`keepTabChats`).
 */
export function abandonedSideChats(
  conversations: readonly Pick<Conversation, "id" | "kind" | "sideOf" | "tabChat">[],
  referenced: ReadonlySet<string>,
): string[] {
  const sessions = new Set(conversations.filter((conversation) => conversation.kind === "session").map((conversation) => conversation.id));
  return conversations.filter((conversation) => conversation.sideOf && !conversation.tabChat && sessions.has(conversation.sideOf) && !referenced.has(conversation.id))
    .map((conversation) => conversation.id);
}
