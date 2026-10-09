import type { Conversation } from "@/ipc/generated";

/** Only a loaded session's unreferenced side chats belong to tab cleanup. */
export function abandonedSideChats(
  conversations: readonly Pick<Conversation, "id" | "kind" | "sideOf">[],
  referenced: ReadonlySet<string>,
): string[] {
  const sessions = new Set(conversations.filter((conversation) => conversation.kind === "session").map((conversation) => conversation.id));
  return conversations.filter((conversation) => conversation.sideOf && sessions.has(conversation.sideOf) && !referenced.has(conversation.id))
    .map((conversation) => conversation.id);
}
