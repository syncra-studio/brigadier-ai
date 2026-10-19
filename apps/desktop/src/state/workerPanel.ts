import { create } from "zustand";

/**
 * A worker's panel an agent asked to show (Q90's `open_in_app`): the session's view on screen
 * opens it in the Workers sidebar, then clears the ask. Only ever asked while the user is on
 * the session and asked to see it.
 */
export const useWorkerPanelAsk = create<{ ask: { conversationId: string; task: string } | null }>()(() => ({ ask: null }));

export function askWorkerPanel(conversationId: string, task: string): void {
  useWorkerPanelAsk.setState({ ask: { conversationId, task } });
}

export function clearWorkerPanelAsk(): void {
  useWorkerPanelAsk.setState({ ask: null });
}
