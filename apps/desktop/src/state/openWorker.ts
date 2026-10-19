import { create } from "zustand";

import { openConversation } from "@/state/actions";

/** A worker to show in its conversation's Workers panel once that conversation is open. */
type OpenWorker = { conversationId: string; taskId: string };

export const useOpenWorker = create<{ request: OpenWorker | null }>(() => ({ request: null }));

/** Opens the conversation `conversationId` with its worker `taskId` shown in the Workers panel. */
export function openWorker(conversationId: string, taskId: string): void {
  useOpenWorker.setState({ request: { conversationId, taskId } });
  openConversation(conversationId);
}

/** Takes the worker waiting to be shown in `conversationId`, if any. */
export function takeOpenWorker(conversationId: string): string | null {
  const { request } = useOpenWorker.getState();
  if (request?.conversationId !== conversationId) return null;
  useOpenWorker.setState({ request: null });
  return request.taskId;
}
