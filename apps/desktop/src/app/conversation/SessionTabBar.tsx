import { Chat, X } from "@openai/apps-sdk-ui/components/Icon";
import {
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  lazy,
  Suspense,
  useEffect,
  useRef,
} from "react";

import { ChatActions } from "@/app/conversation/ChatActions";
import { FileTabView } from "@/app/conversation/FileTabView";
import { DiffGlyph } from "@/components/assistant-ui/elements/diff-glyph";
import { FileTypeIcon } from "@/components/assistant-ui/elements/file-type-icon";
import { Badge } from "@/components/ui/badge";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { useSidebar } from "@/components/ui/sidebar";
import { useDragReorder } from "@/hooks/use-drag-reorder";
import type { Conversation } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import {
  CHAT_TAB,
  closeOtherTabs,
  closeTab,
  closeTabsToTheRight,
  keepTabOpen,
  moveTab,
  type SessionTab,
  selectTab,
  selectTabNumber,
  sessionTabs,
  stepTab,
  tabOrder,
  useSessionTabsOf,
} from "@/state/sessionTabs";
import { useApp } from "@/state/store";

/** The names the daemon gives a conversation until its first message titles it. */
const UNTITLED = new Set(["New session", "New chat"]);

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

export function tabTitle(tab: SessionTab): string {
  if (tab.kind === "review") {
    return tab.target.type === "all" ? "Review" : `${baseName(tab.target.path)} (diff)`;
  }
  return baseName(tab.path);
}

/** A tab's look: 32px and rounded, the one in front lifted, the others muted. */
const TAB =
  "group/tab relative flex h-8 min-w-session-tab-min flex-[0_1_var(--spacing-panel-tab)] items-center gap-1 rounded-lg ps-2 pe-1 text-sm select-none";

/** Mouse down with the middle button would start autoscroll. */
function noAutoscroll(event: ReactMouseEvent): void {
  if (event.button === 1) event.preventDefault();
}

/** Where focus is in a pane that keeps its own tab keys (a terminal, a browser page). */
function inOwnPane(target: EventTarget | null): boolean {
  return (
    target instanceof Element &&
    target.closest('[data-slot="terminal-pane"], [data-terminal-menu], [data-pane="browser"]') !==
      null
  );
}

/**
 * The tab keys while a session shows: ⌘1 Chat and ⌘2–⌘9 the others, ⌘W closes the tab in
 * front (never Chat), ⌃Tab or ⌘⇧] the next and ⌃⇧Tab or ⌘⇧[ the previous (Ctrl for ⌘ off
 * macOS). A terminal or browser page with focus keeps them for its own tabs.
 */
function useTabKeys(conversationId: string): void {
  const mac = useApp((s) => s.info?.platform === "macos");
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || inOwnPane(event.target)) return;
      const command = mac ? event.metaKey : event.ctrlKey;
      const other = mac ? event.ctrlKey : event.metaKey;
      if (event.ctrlKey && !event.metaKey && !event.altKey && event.code === "Tab") {
        event.preventDefault();
        stepTab(conversationId, event.shiftKey ? -1 : 1);
        return;
      }
      if (!command || other || event.altKey) return;
      if (event.shiftKey && (event.code === "BracketLeft" || event.code === "BracketRight")) {
        event.preventDefault();
        stepTab(conversationId, event.code === "BracketLeft" ? -1 : 1);
        return;
      }
      if (event.shiftKey) return;
      const digit = /^Digit([1-9])$/.exec(event.code);
      if (digit) {
        event.preventDefault();
        selectTabNumber(conversationId, Number(digit[1]));
        return;
      }
      if (event.code === "KeyW") {
        const { active } = sessionTabs(conversationId);
        if (active === CHAT_TAB) return;
        event.preventDefault();
        closeTab(conversationId, active);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [conversationId, mac]);
}

/** A tab's right-click menu: Keep open (a preview), Close, Close others, Close to the right. */
function TabMenu({
  conversationId,
  tab,
  children,
}: {
  conversationId: string;
  tab: SessionTab | null;
  children: ReactNode;
}) {
  const id = tab?.id ?? CHAT_TAB;
  const order = tabOrder(sessionTabs(conversationId));
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent>
        {tab?.kind === "file" && tab.preview && (
          <>
            <ContextMenuItem onSelect={() => keepTabOpen(conversationId, id)}>
              Keep open
            </ContextMenuItem>
            <ContextMenuSeparator />
          </>
        )}
        <ContextMenuItem disabled={!tab} onSelect={() => closeTab(conversationId, id)}>
          Close
        </ContextMenuItem>
        <ContextMenuItem
          disabled={order.length <= (tab ? 2 : 1)}
          onSelect={() => closeOtherTabs(conversationId, id)}
        >
          Close others
        </ContextMenuItem>
        <ContextMenuItem
          disabled={order.indexOf(id) === order.length - 1}
          onSelect={() => closeTabsToTheRight(conversationId, id)}
        >
          Close to the right
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

/**
 * A session's top bar as tabs: Chat first (its title, its ⋯ chat actions; never closed or
 * moved), then the files and the Review tab it opened, which shrink to their least and then
 * scroll. Click to show, drag to reorder, middle-click to close; a preview tab is in italic
 * until double-clicked. `children` sit at the end: the summary toggle and the panel buttons'
 * room.
 */
export function SessionTabBar({
  conversation,
  onRename,
  children,
}: {
  conversation: Conversation;
  onRename: () => void;
  children?: ReactNode;
}) {
  const { state } = useSidebar();
  const id = conversation.id;
  const { tabs, active } = useSessionTabsOf(id);
  const connection = useApp((s) => s.connection.status);
  const archived = conversation.lifecycle === "archived";
  const strip = useRef<HTMLDivElement>(null);
  useTabKeys(id);
  const { listRef, shown, dragging, grip } = useDragReorder<SessionTab, HTMLDivElement>({
    items: tabs,
    idOf: (tab) => tab.id,
    rowSelector: "[data-session-tab]",
    onMove: (tab, to) => moveTab(id, tab, to),
    axis: "x",
  });

  // The tab in front scrolls into view.
  useEffect(() => {
    const element = strip.current?.querySelector<HTMLElement>(`[data-tab-id="${CSS.escape(active)}"]`);
    element?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [active]);

  const title = UNTITLED.has(conversation.title) ? "Chat" : conversation.title;
  // On the button's release: the webview may never send `auxclick`.
  const middleClose = (tab: string) => (event: ReactMouseEvent) => {
    if (event.button !== 1) return;
    event.preventDefault();
    closeTab(id, tab);
  };

  return (
    <header
      data-tauri-drag-region
      data-slot="session-tabs"
      className={cn(
        "h-titlebar ease-sidebar flex shrink-0 items-center gap-1 ps-2 pe-1 transition-[padding] duration-300 motion-reduce:transition-none",
        state === "collapsed" && "ps-titlebar-clear",
      )}
    >
      <div
        ref={strip}
        role="tablist"
        aria-label="Session tabs"
        tabIndex={-1}
        data-tauri-drag-region
        className="hide-scrollbar flex min-w-0 flex-1 scroll-px-1 items-center gap-0.5 overflow-x-auto"
        onKeyDown={(event) => {
          if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
          if (!(event.target instanceof HTMLElement) || event.target.getAttribute("role") !== "tab")
            return;
          event.preventDefault();
          const order = tabOrder(sessionTabs(id));
          const at = order.indexOf(active);
          const rtl = getComputedStyle(event.currentTarget).direction === "rtl";
          const forward = (event.key === "ArrowRight") !== rtl;
          const next =
            event.key === "Home"
              ? 0
              : event.key === "End"
                ? order.length - 1
                : (at + (forward ? 1 : -1) + order.length) % order.length;
          const target = order[next]!;
          selectTab(id, target);
          requestAnimationFrame(() =>
            strip.current
              ?.querySelector<HTMLElement>(`[data-tab-id="${CSS.escape(target)}"] [role="tab"]`)
              ?.focus(),
          );
        }}
      >
        <TabMenu conversationId={id} tab={null}>
          <div
            data-tab-id={CHAT_TAB}
            data-active={active === CHAT_TAB || undefined}
            className={cn(
              TAB,
              active === CHAT_TAB
                ? "bg-panel-tab shadow-panel-tab"
                : "text-toolbar-foreground hover:bg-toolbar-hover",
            )}
          >
            <button
              type="button"
              role="tab"
              onClick={() => selectTab(id, CHAT_TAB)}
              aria-selected={active === CHAT_TAB}
              tabIndex={active === CHAT_TAB ? 0 : -1}
              title={conversation.title}
              className="focus-visible:ring-ring flex h-full min-w-0 flex-1 items-center gap-1.5 rounded-sm outline-none focus-visible:ring-1"
            >
              <Chat aria-hidden className="size-icon-sm shrink-0" />
              <span className="min-w-0 truncate">{title}</span>
            </button>
            {archived && (
              <Badge
                variant="outline"
                className="shrink-0"
                title="Archived: restore it from Settings → Archived chats to continue."
              >
                Archived
              </Badge>
            )}
            <ChatActions
              conversation={conversation}
              onRename={onRename}
              compact
              className={cn(
                "shrink-0 opacity-0 transition-opacity group-hover/tab:opacity-100 focus-visible:opacity-100 aria-expanded:opacity-100",
                active === CHAT_TAB && "opacity-100",
              )}
            />
          </div>
        </TabMenu>
        <div ref={listRef} className="contents">
          {shown.map((tab) => {
            const index = tabs.findIndex((entry) => entry.id === tab.id);
            const selected = tab.id === active;
            const name = tabTitle(tab);
            const handlers = grip(tab.id, index);
            return (
              <TabMenu key={tab.id} conversationId={id} tab={tab}>
                <div
                  data-session-tab
                  data-tab-id={tab.id}
                  data-active={selected || undefined}
                  data-dragging={dragging === tab.id || undefined}
                  className={cn(
                    TAB,
                    selected
                      ? "bg-panel-tab shadow-panel-tab"
                      : "text-toolbar-foreground hover:bg-toolbar-hover",
                    "data-dragging:z-10 data-dragging:opacity-80",
                  )}
                >
                  {/* The tab itself is the drag handle; Alt+←/→ moves it from the keyboard. */}
                  <button
                    type="button"
                    role="tab"
                    aria-selected={selected}
                    tabIndex={selected ? 0 : -1}
                    title={tab.kind === "file" ? tab.path : name}
                    onPointerDown={handlers.onPointerDown}
                    onPointerMove={handlers.onPointerMove}
                    onPointerUp={handlers.onPointerUp}
                    onPointerCancel={handlers.onPointerCancel}
                    onMouseDown={noAutoscroll}
                    onMouseUp={middleClose(tab.id)}
                    onClick={() => selectTab(id, tab.id)}
                    onDoubleClick={() => keepTabOpen(id, tab.id)}
                    onKeyDown={(event) => {
                      if (event.altKey) handlers.onKeyDown(event);
                    }}
                    className="focus-visible:ring-ring flex h-full min-w-0 flex-1 items-center gap-1.5 rounded-sm outline-none focus-visible:ring-1"
                  >
                    {tab.kind === "file" ? (
                      <FileTypeIcon name={tab.path} className="size-icon-sm shrink-0" />
                    ) : (
                      <DiffGlyph className="size-icon-sm shrink-0" />
                    )}
                    <span className={cn("min-w-0 truncate", tab.kind === "file" && tab.preview && "italic")}>
                      {name}
                    </span>
                  </button>
                  <button
                    type="button"
                    aria-label={`Close ${name}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      closeTab(id, tab.id);
                    }}
                    className={cn(
                      "hover:bg-toolbar-hover focus-visible:ring-ring flex size-5 shrink-0 items-center justify-center rounded-full opacity-60 outline-none hover:opacity-100 focus-visible:ring-1",
                      !selected && "invisible group-focus-within/tab:visible group-hover/tab:visible",
                    )}
                  >
                    <X aria-hidden className="size-icon-xs" />
                  </button>
                </div>
              </TabMenu>
            );
          })}
        </div>
      </div>
      {connection !== "connected" && (
        <Badge variant="warning" role="status" className="shrink-0">
          {connection === "connecting" ? "Connecting to core…" : "Reconnecting to core…"}
        </Badge>
      )}
      {children}
    </header>
  );
}

const ReviewTab = lazy(() =>
  import("@/app/conversation/ReviewTab").then((module) => ({ default: module.ReviewTab })),
);

/**
 * The session's open tabs over its conversation, which stays as it was (scrolled, its draft
 * typed) under them: each tab keeps its place too, only the one in front shows. Nothing shows
 * while Chat is in front.
 */
export function SessionTabViews({ conversationId }: { conversationId: string }) {
  const { tabs, active } = useSessionTabsOf(conversationId);
  return (
    <>
      {tabs.map((tab) => {
        const shown = tab.id === active;
        return (
          <div
            key={tab.id}
            role="tabpanel"
            aria-label={tabTitle(tab)}
            inert={!shown}
            className={cn("bg-background absolute inset-0 flex flex-col", !shown && "invisible")}
          >
            {tab.kind === "file" ? (
              <FileTabView conversationId={conversationId} tab={tab} active={shown} />
            ) : (
              <Suspense fallback={null}>
                <ReviewTab conversationId={conversationId} target={tab.target} active={shown} />
              </Suspense>
            )}
          </div>
        );
      })}
    </>
  );
}
