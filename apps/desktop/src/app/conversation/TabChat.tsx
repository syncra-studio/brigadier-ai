import { createContext, type ReactNode, type RefObject, useContext, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { ConversationView } from "@/app/ConversationView";
import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { Chat, ChatCompose, MinimizeRight, X } from "@/components/icons";
import { browserRead, request } from "@/ipc/client";
import { cn } from "@/lib/utils";
import { loadConversation } from "@/state/actions";
import { useRowActivity } from "@/state/activity";
import { type BoardStore, BoardStoreContext, createSideBoard, disposeSideBoard } from "@/state/board";
import type { TabChatState } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import {
  type ChatTab,
  chatTarget,
  newTabChat,
  prepareBeforeSend,
  setTabChatShare,
  setTabChatView,
  showTabChat,
} from "@/state/tabChats";

/** What the tab's chat button says. */
export function chatAbout(tab: ChatTab): string {
  return tab.kind === "file" ? "Chat about this file" : "Chat about this page";
}

/**
 * The tab's chat button for a toolbar of the tab's own (a browser tab's where its page draws
 * over the app, so nothing can float above it: not macOS). Outside a tab with a chat it shows
 * nothing.
 */
const ToolbarButton = createContext<ReactNode>(null);
export function TabChatToolbarButton() {
  return useContext(ToolbarButton);
}

/**
 * A page, file or browser tab with its own chat: the tab's content, and while the chat is
 * open, a resizable split with the chat in the half at the end. Folded, the chat is a strip
 * at the tab's edge; hidden, a round button brings it back as it was.
 */
export function TabWithChat({ conversationId, tab, children }: { conversationId: string; tab: ChatTab; children: ReactNode }) {
  const target = chatTarget(tab);
  const chat = target ? tab.chat : undefined;
  const view = chat?.view ?? "hidden";
  const row = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  // On macOS a browser tab's page lies beneath the app, so its button floats over the page too.
  const mac = useApp((s) => s.info?.platform === "macos");
  const inToolbar = tab.kind === "browser" && !mac;
  const hidden = target && view === "hidden";
  const button = hidden ? <ChatButton conversationId={conversationId} tab={tab} chat={chat} floating={!inToolbar} /> : null;
  return (
    <ToolbarButton.Provider value={inToolbar ? button : null}>
      <div ref={row} data-slot="tab-with-chat" className="relative flex min-h-0 min-w-0 flex-1">
        <div
          ref={content}
          data-slot="tab-content"
          className="relative flex min-h-0 min-w-0 flex-col"
          style={{ flex: view === "split" && chat ? `0 0 ${(1 - chat.share) * 100}%` : "1 1 0%" }}
        >
          {children}
          {tab.kind !== "browser" && button && <div className="absolute end-4 bottom-4 z-20">{button}</div>}
          {tab.kind === "browser" && !inToolbar && button && <AbovePage anchor={content}>{button}</AbovePage>}
        </div>
        {chat && view === "split" && (
          <>
            <SplitDivider conversationId={conversationId} tabId={tab.id} share={chat.share} row={row} />
            <div data-slot="tab-chat" className="border-border flex min-h-0 min-w-0 flex-1 flex-col border-s">
              <TabChatPane conversationId={conversationId} tab={tab} chat={chat} />
            </div>
          </>
        )}
        {chat && view === "minimized" && <ChatStrip conversationId={conversationId} tabId={tab.id} chat={chat} />}
      </div>
    </ToolbarButton.Provider>
  );
}

/** The round button that opens the chat; a dot while the hidden chat works or has news. */
function ChatButton({ conversationId, tab, chat, floating }: { conversationId: string; tab: ChatTab; chat: TabChatState | undefined; floating: boolean }) {
  const news = useNews(chat);
  return (
    <div data-slot="tab-chat-button">
      <TooltipIconButton
        tooltip={chatAbout(tab)}
        side={floating ? "left" : "bottom"}
        size={floating ? "icon-lg" : "icon-sm"}
        variant={floating ? "secondary" : "ghost"}
        className={cn("relative rounded-full", floating && "shadow-hairline bg-composer size-10 backdrop-blur-lg")}
        onClick={() => showTabChat(conversationId, tab.id)}
      >
        <Chat />
        {news && <span data-slot="tab-chat-dot" aria-label="New in the chat" className="bg-primary absolute end-1 top-1 size-2 rounded-full" />}
      </TooltipIconButton>
    </div>
  );
}

/**
 * Floats `children` in the bottom end corner of `anchor`, over a browser tab's page beneath the
 * app. The app's own content is cut where the page shows, so it goes at the end of the body
 * instead, and says where it is (`data-above-page`) so clicks there stay the app's. Nothing
 * shows while the tab doesn't.
 */
function AbovePage({ anchor, children }: { anchor: RefObject<HTMLDivElement | null>; children: ReactNode }) {
  const [corner, setCorner] = useState<{ end: number; bottom: number } | null>(null);
  useEffect(() => {
    let frame = 0;
    let placed = "";
    // The tab moves and resizes with the panels around it: followed every frame.
    const follow = () => {
      const element = anchor.current;
      const box = element && !element.closest("[inert]") ? element.getBoundingClientRect() : null;
      const rtl = document.dir === "rtl";
      const next = box && box.width > 0 && box.height > 0
        ? { end: (rtl ? box.left : window.innerWidth - box.right) + 16, bottom: window.innerHeight - box.bottom + 16 }
        : null;
      const key = JSON.stringify(next);
      if (key !== placed) {
        placed = key;
        setCorner(next);
      }
      frame = requestAnimationFrame(follow);
    };
    follow();
    return () => cancelAnimationFrame(frame);
  }, [anchor]);
  if (!corner) return null;
  return createPortal(
    <div data-above-page className="fixed z-20" style={{ insetInlineEnd: corner.end, bottom: corner.bottom }}>
      {children}
    </div>,
    document.body,
  );
}

/** Whether the chat works now, or changed since it was last in view. */
function useNews(chat: TabChatState | undefined): boolean {
  const working = useRowActivity(chat?.id ?? "").running;
  const changedAt = useApp((s) => (chat ? s.conversations[chat.id]?.updatedAtMs : undefined));
  if (!chat) return false;
  return working || (changedAt !== undefined && chat.seenAt !== undefined && changedAt > chat.seenAt);
}

/** The split's divider: drag it, or use the arrow keys, to share the width differently. */
function SplitDivider({ conversationId, tabId, share, row }: { conversationId: string; tabId: string; share: number; row: React.RefObject<HTMLDivElement | null> }) {
  const [dragging, setDragging] = useState(false);
  // Read by the moves, which can come before the press has drawn.
  const held = useRef(false);
  const shareAt = (x: number) => {
    const box = row.current?.getBoundingClientRect();
    if (!box || box.width <= 0) return share;
    const rtl = row.current ? getComputedStyle(row.current).direction === "rtl" : false;
    const fromEnd = rtl ? x - box.left : box.right - x;
    return fromEnd / box.width;
  };
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize chat"
      aria-valuenow={Math.round(share * 100)}
      aria-valuemin={25}
      aria-valuemax={75}
      tabIndex={0}
      data-slot="tab-chat-divider"
      data-resizing={dragging || undefined}
      className="group/resize w-resize-handle relative z-10 -mx-[calc(var(--spacing-resize-handle)/2)] flex shrink-0 cursor-col-resize justify-center outline-none"
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        held.current = true;
        setDragging(true);
      }}
      onPointerMove={(event) => {
        if (held.current) setTabChatShare(conversationId, tabId, shareAt(event.clientX));
      }}
      onPointerUp={(event) => {
        if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
        held.current = false;
        setDragging(false);
      }}
      onPointerCancel={() => { held.current = false; setDragging(false); }}
      onKeyDown={(event) => {
        const step = event.key === "ArrowLeft" ? 0.05 : event.key === "ArrowRight" ? -0.05 : 0;
        if (!step) return;
        event.preventDefault();
        const rtl = getComputedStyle(event.currentTarget).direction === "rtl";
        setTabChatShare(conversationId, tabId, share + (rtl ? -step : step));
      }}
    >
      <span className="bg-input w-px opacity-0 transition-opacity duration-150 group-hover/resize:opacity-100 group-focus-visible/resize:opacity-100 group-data-resizing/resize:opacity-100" />
    </div>
  );
}

/** The folded chat: a strip at the tab's edge with its title and whether it works. */
function ChatStrip({ conversationId, tabId, chat }: { conversationId: string; tabId: string; chat: TabChatState }) {
  const title = useApp((s) => s.conversations[chat.id]?.title ?? "Chat");
  const working = useRowActivity(chat.id).running;
  const news = useNews(chat);
  return (
    <button
      type="button"
      data-slot="tab-chat-strip"
      aria-label={`Show chat: ${title}${working ? ", working" : ""}`}
      onClick={() => showTabChat(conversationId, tabId)}
      className="border-border hover:bg-accent text-muted-foreground flex w-9 shrink-0 flex-col items-center gap-2 border-s py-3 text-xs"
    >
      <span className="relative">
        <Chat className="size-4" />
        {news && <span data-slot="tab-chat-dot" className="bg-primary absolute -end-1 -top-1 size-2 rounded-full" />}
      </span>
      <span className="min-h-0 truncate [writing-mode:vertical-rl]">{title}</span>
      {working && <span className="[writing-mode:vertical-rl]">Working…</span>}
    </button>
  );
}

/** The chat half: its header (Minimize, Hide, New chat) over the chat itself. */
function TabChatPane({ conversationId, tab, chat }: { conversationId: string; tab: ChatTab; chat: TabChatState }) {
  const title = useApp((s) => s.conversations[chat.id]?.title ?? "Chat");
  const seenAt = () => useApp.getState().conversations[chat.id]?.updatedAtMs ?? Date.now();
  return (
    <>
      <div data-slot="tab-chat-header" className="h-titlebar flex shrink-0 items-center gap-1 px-2">
        <span className="text-foreground min-w-0 flex-1 truncate ps-1 text-sm font-medium">{title}</span>
        <TooltipIconButton tooltip="New chat" onClick={() => newTabChat(conversationId, tab.id)}>
          <ChatCompose />
        </TooltipIconButton>
        <TooltipIconButton tooltip="Minimize" onClick={() => setTabChatView(conversationId, tab.id, "minimized", seenAt())}>
          <MinimizeRight />
        </TooltipIconButton>
        <TooltipIconButton tooltip="Hide chat" onClick={() => setTabChatView(conversationId, tab.id, "hidden", seenAt())}>
          <X />
        </TooltipIconButton>
      </div>
      <TabChatThread key={chat.id} conversationId={conversationId} tab={tab} chatId={chat.id} />
    </>
  );
}

/** The chat's thread and composer, opened (or made) for the tab. */
function TabChatThread({ conversationId, tab, chatId }: { conversationId: string; tab: ChatTab; chatId: string }) {
  const [store, setStore] = useState<BoardStore | null>(null);
  const [error, setError] = useState<string | null>(null);
  const target = chatTarget(tab);
  const targetKey = JSON.stringify(target);
  useEffect(() => {
    if (!target) return;
    let live = true;
    let board: BoardStore | null = null;
    request({ method: "openTabChat", conversationId, tabId: tab.id, target, chatId })
      .then(({ conversation }) => {
        if (!live) return;
        useApp.setState((state) => ({ conversations: { ...state.conversations, [conversation.id]: conversation } }));
        board = createSideBoard(conversation.id);
        setStore(board);
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
      if (board) disposeSideBoard(board);
    };
    // The target, by value.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [conversationId, tab.id, chatId, targetKey]);
  // A browser tab's chat reads the page as it is each time the user writes.
  const browserTab = tab.kind === "browser" ? tab : null;
  useEffect(() => {
    if (!browserTab) return;
    return prepareBeforeSend(chatId, async () => {
      const page = await browserRead(browserTab.id).catch(() => ({ url: browserTab.url, title: browserTab.title, text: "", selection: "" }));
      await request({ method: "setTabPage", chatId, ...page });
    });
  }, [chatId, browserTab]);
  if (error) return <p role="alert" className="text-destructive p-4 text-sm">{error}</p>;
  if (!store) return null;
  return (
    <BoardStoreContext.Provider value={store}>
      <ConversationView selection={{ type: "conversation", id: chatId }} embedded />
    </BoardStoreContext.Provider>
  );
}
