import { request } from "@/ipc/client";
import { useApp } from "@/state/store";

/** Each conversation's side chat while its tab is open, so closing the tab deletes it. */
const open = new Map<string, string>();

export function noteSideChat(conversationId: string, sideChatId: string): void {
  open.set(conversationId, sideChatId);
}

/** Deletes the conversation's side chat: side chats are temporary. */
export function closeSideChat(conversationId: string): void {
  const id = open.get(conversationId);
  if (!id) return;
  open.delete(conversationId);
  useApp.setState((state) => {
    const { [id]: _closed, ...threads } = state.threads;
    return { threads };
  });
  request({ method: "delete", ids: [id] }).catch(
    (error: unknown) => console.error("deleting the side chat failed", error),
  );
}
