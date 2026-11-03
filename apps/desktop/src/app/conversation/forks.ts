import type { Conversation, Setup } from "@/ipc/generated";

/** The folder a session works in, once it has one (as the core reads it). */
export function folderOf(setup: Setup | null): string | null {
  if (setup?.type !== "session") return null;
  return setup.environment.type === "localCheckout" ? setup.repo : setup.environment.path;
}

/** The fork, or the source, `conversation` shares its folder with: open ones only. */
export function folderSharers(conversation: Conversation, all: Record<string, Conversation>): Conversation[] {
  const folder = folderOf(conversation.setup);
  if (!folder) return [];
  const from = conversation.forkedFrom?.conversationId;
  return Object.values(all).filter(
    (other) =>
      other.id !== conversation.id &&
      other.lifecycle !== "archived" &&
      (other.id === from || other.forkedFrom?.conversationId === conversation.id) &&
      folderOf(other.setup) === folder,
  );
}

/** `list` with each fork moved right under its source, when that is in `list` too. */
export function forksUnderSources(list: Conversation[]): Conversation[] {
  const ids = new Set(list.map((conversation) => conversation.id));
  const sourceIn = (conversation: Conversation) => {
    const from = conversation.forkedFrom?.conversationId;
    return from !== undefined && from !== conversation.id && ids.has(from) ? from : null;
  };
  const forks = new Map<string, Conversation[]>();
  for (const conversation of list) {
    const from = sourceIn(conversation);
    if (from) forks.set(from, [...(forks.get(from) ?? []), conversation]);
  }
  if (forks.size === 0) return list;
  const ordered: Conversation[] = [];
  const place = (conversation: Conversation) => {
    ordered.push(conversation);
    for (const fork of forks.get(conversation.id) ?? []) place(fork);
  };
  for (const conversation of list) if (!sourceIn(conversation)) place(conversation);
  return ordered;
}
