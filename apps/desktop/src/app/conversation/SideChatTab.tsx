import { useEffect, useState } from "react";

import { ConversationView } from "@/app/ConversationView";
import { request } from "@/ipc/client";
import { loadConversation } from "@/state/actions";
import {
  type BoardStore,
  BoardStoreContext,
  createSideBoard,
  disposeSideBoard,
} from "@/state/board";
import { changeSessionTab, type SideChatTabState } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

/**
 * The Side chat tab (⌥⌘S): a temporary chat beside the conversation, for questions about it
 * that stay out of its thread. Each tab keeps its own conversation across closes and restarts.
 */
export function SideChatTab({ conversationId, tab }: { conversationId: string; tab: SideChatTabState }) {
  const firstMessage = useApp((state) => tab.conversationId ? state.threads[tab.conversationId]?.items.find((item) => item.role === "user")?.text : undefined);
  useEffect(() => {
    if (!tab.title && firstMessage) changeSessionTab(conversationId, tab.id, (current) => current.kind === "sideChat" ? { ...current, title: firstMessage.trim().split("\n")[0]!.slice(0, 120) } : current);
  }, [conversationId, tab.id, tab.title, firstMessage]);
  const [side, setSide] = useState<{ id: string; store: BoardStore } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    let store: BoardStore | null = null;
    request({ method: "openSideChat", conversationId, ...(tab.conversationId ? { sideChatId: tab.conversationId } : {}) })
      .then(({ conversation }) => {
        if (!live) return;
        useApp.setState((state) => ({
          conversations: { ...state.conversations, [conversation.id]: conversation },
        }));
        changeSessionTab(conversationId, tab.id, (current) => current.kind === "sideChat" ? { ...current, conversationId: conversation.id } : current);
        store = createSideBoard(conversation.id);
        setSide({ id: conversation.id, store });
        setError(null);
        void loadConversation(conversation.id).catch((cause: unknown) => {
          if (live) setError(cause instanceof Error ? cause.message : String(cause));
        });
      })
      .catch((cause: unknown) => {
        if (live) setError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      live = false;
      if (store) disposeSideBoard(store);
    };
  }, [conversationId, tab.id, tab.conversationId]);

  if (error) {
    return (
      <p role="alert" className="text-destructive p-4 text-sm">
        {error}
      </p>
    );
  }
  if (!side) return null;
  return (
    <BoardStoreContext.Provider value={side.store}>
      <ConversationView selection={{ type: "conversation", id: side.id }} embedded />
    </BoardStoreContext.Provider>
  );
}
