import { Chat, Globe, Terminal, Plus, Document, X } from "@openai/apps-sdk-ui/components/Icon";
import {
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  useCallback,
  lazy,
  Suspense,
  useEffect,
  useRef,
} from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useCheckoutRoot } from "@/components/assistant-ui/file-links";
import { TitlebarButton } from "@/components/titlebar-button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuShortcut, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { NEW_TAB_MENU, sessionTabKey } from "@/state/sessionTabKeys";
import { shortcutLabel } from "@/app/conversation/SidePanel";
import { takePaneClose } from "@/state/closedPanes";
import { undoTabClose } from "@/state/terminalPlaces";
import { ChatActions } from "@/app/conversation/ChatActions";
import { PreviewChip } from "@/app/conversation/PreviewChip";
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
import { useDragReorder } from "@/hooks/use-drag-reorder";
import type { Conversation } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import {
  CHAT_TAB,
  newSessionTab,
  reopenTab,
  type NewTabKind,
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
  useTabCloseAsk,
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
  if (tab.kind === "browser") return tab.title || tab.url || "New browser";
  if (tab.kind === "terminal") return tab.title || (tab.cwd ? baseName(tab.cwd) : "Terminal");
  if (tab.kind === "sideChat") return tab.title || "Side chat";
  if (tab.kind === "document") return tab.name;
  return baseName(tab.path);
}

/** A tab's look: 32px and rounded, the one in front lifted, the others muted. */
const TAB =
  "group/tab relative flex h-8 w-panel-tab min-w-session-tab-min flex-[0_1_var(--spacing-panel-tab)] items-center gap-1 rounded-lg ps-2 pe-1 text-sm select-none";

/** Mouse down with the middle button would start autoscroll. */
function noAutoscroll(event: ReactMouseEvent): void {
  if (event.button === 1) event.preventDefault();
}

/** Session keys own main tabs. The bottom pane keeps Cmd+W and bracket navigation; off macOS a focused
 * terminal keeps its shell Ctrl keys. Cmd+T always creates a main tab on macOS. */
function useTabKeys(conversationId: string, create: (kind: NewTabKind) => void): void {
  const mac = useApp((s) => s.info?.platform === "macos");
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return;
      const target = event.target instanceof Element ? event.target : document.activeElement;
      if (!mac && target?.closest('[data-slot="terminal-pane"], [data-main-terminal]')) return;
      if (target?.closest('[data-slot="terminal-pane"]') && ["KeyW", "BracketLeft", "BracketRight"].includes(event.code)) return;
      const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (command && !event.altKey && event.shiftKey && event.code === "KeyT") {
        event.preventDefault(); event.stopPropagation();
        const closed = takePaneClose(conversationId);
        if (closed === "tab") reopenTab(conversationId, useApp.getState().conversations[conversationId]?.lifecycle !== "archived");
        else if (closed === "terminal") undoTabClose(`conv:${conversationId}`);
        return;
      }
      const action = sessionTabKey(event, mac);
      if (!action) return;
      event.preventDefault(); event.stopPropagation();
      if (action.type === "new") create(action.kind);
      if (action.type === "close") closeTab(conversationId, sessionTabs(conversationId).active);
      if (action.type === "number") selectTabNumber(conversationId, action.number);
      if (action.type === "step") stepTab(conversationId, action.step);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [conversationId, mac, create]);
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
 * until double-clicked. While the thread keeps something running for the user, its chip
 * ("web · Running · Stop") follows. `children` sit at the end: the summary toggle and the panel
 * buttons' room.
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
  const id = conversation.id;
  const confirmClose = useTabCloseAsk((state) => state.confirm);
  const { tabs, active } = useSessionTabsOf(id);
  const connection = useApp((s) => s.connection.status);
  const archived = conversation.lifecycle === "archived";
  const strip = useRef<HTMLDivElement>(null);
  const mac = useApp((s) => s.info?.platform === "macos");
  const cwd = useCheckoutRoot();
  const create = useCallback((kind: NewTabKind) => {
    if (archived && (kind === "terminal" || kind === "sideChat")) return;
    newSessionTab(id, kind, cwd);
  }, [id, archived, cwd]);
  useTabKeys(id, create);
  const { listRef, shown, dragging, grip } = useDragReorder<SessionTab, HTMLDivElement>({
    items: tabs,
    idOf: (tab) => tab.id,
    rowSelector: "[data-session-tab]",
    onMove: (tab, to) => moveTab(id, tab, to),
    axis: "x",
  });

  // The tab in front scrolls into view, and stays there as the strip's room changes.
  useEffect(() => {
    const list = strip.current;
    if (!list) return;
    const reveal = () =>
      list
        .querySelector<HTMLElement>(`[data-tab-id="${CSS.escape(active)}"]`)
        ?.scrollIntoView({ block: "nearest", inline: "nearest" });
    reveal();
    const observer = new ResizeObserver(reveal);
    observer.observe(list);
    return () => observer.disconnect();
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
      className="session-titlebar-divider relative h-titlebar ease-sidebar ps-clear-2 flex shrink-0 items-center gap-1 pe-1 transition-[padding] duration-300 motion-reduce:transition-none"
    >
      <div
        role="tablist"
        aria-label="Session tabs"
        tabIndex={-1}
        data-tauri-drag-region
        className="flex min-w-0 shrink items-center gap-0.5"
        onKeyDown={(event) => {
          // Alt+←/→ moved the tab itself (its drag handle's keys).
          if (event.defaultPrevented || event.altKey) return;
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
          requestAnimationFrame(() => {
            strip.current?.parentElement
              ?.querySelector<HTMLElement>(`[data-tab-id="${CSS.escape(target)}"] [role="tab"]`)
              ?.focus();
          });
        }}
      >
        <ContextMenu>
          <ContextMenuTrigger asChild>
            <div
              data-tab-id={CHAT_TAB}
              data-active={active === CHAT_TAB || undefined}
              className={cn(
                TAB,
                "max-w-session-title shrink-0",
                active === CHAT_TAB
                  ? "bg-panel-tab shadow-panel-tab"
                  : "text-toolbar-foreground hover:bg-toolbar-hover",
              )}
            >
              <button
                type="button"
                role="tab"
                aria-selected={active === CHAT_TAB}
                tabIndex={active === CHAT_TAB ? 0 : -1}
                title={conversation.title}
                onClick={() => selectTab(id, CHAT_TAB)}
                onDoubleClick={archived ? undefined : onRename}
                className="flex h-full min-w-0 flex-1 items-center gap-1.5 rounded-sm"
              >
                <Chat aria-hidden className="size-icon-sm shrink-0" />
                <span className="min-w-0 truncate">{title}</span>
              </button>
              {archived && <Badge variant="outline" className="shrink-0">Archived</Badge>}
              <ChatActions
                conversation={conversation}
                onRename={onRename}
                compact
                className="shrink-0 opacity-0 transition-opacity group-hover/tab:opacity-100 focus-visible:opacity-100 aria-expanded:opacity-100"
              />
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem disabled={archived} onSelect={onRename}>Rename</ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
        <div
          ref={strip}
          data-tauri-drag-region
          className="hide-scrollbar flex min-w-0 shrink scroll-px-1 items-center gap-0.5 overflow-x-auto"
        >
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
                      className="flex h-full min-w-0 flex-1 items-center gap-1.5 rounded-sm"
                    >
                      {tab.kind === "file" ? (
                        <FileTypeIcon name={tab.path} className="size-icon-sm shrink-0" />
                      ) : tab.kind === "browser" ? <Globe className="size-icon-sm shrink-0" />
                        : tab.kind === "terminal" ? <Terminal className="size-icon-sm shrink-0" />
                        : tab.kind === "sideChat" ? <Chat className="size-icon-sm shrink-0" />
                        : tab.kind === "document" ? <Document className="size-icon-sm shrink-0" /> : (
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
                        "hover:bg-toolbar-hover flex size-5 shrink-0 items-center justify-center rounded-full opacity-60 hover:opacity-100",
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
      </div>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <TitlebarButton tooltip="New tab" aria-label="New tab"><Plus /></TitlebarButton>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {NEW_TAB_MENU.filter((item) => !archived || (item.kind !== "terminal" && item.kind !== "sideChat")).map((item) => (
            <DropdownMenuItem key={item.kind} onSelect={() => create(item.kind)}>
              {item.label}<DropdownMenuShortcut>{shortcutLabel(item.shortcut, mac)}</DropdownMenuShortcut>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      {connection !== "connected" && (
        <Badge variant="warning" role="status" className="shrink-0">
          {connection === "connecting" ? "Connecting to core…" : "Reconnecting to core…"}
        </Badge>
      )}
      <PreviewChip conversationId={id} />
      <div data-tauri-drag-region className="min-w-0 flex-1 self-stretch" />
      {children}
      <Dialog open={confirmClose !== null} onOpenChange={(open) => { if (!open) useTabCloseAsk.setState({ confirm: null }); }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Close unsaved files?</DialogTitle>
            <DialogDescription>These files have unsaved changes. Keep them open to save your work, or close them without saving.</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={() => useTabCloseAsk.setState({ confirm: null })}>Keep open</Button>
            <Button variant="destructive" onClick={() => {
              useTabCloseAsk.setState({ confirm: null });
              confirmClose?.();
            }}>Close without saving</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </header>
  );
}

const FileTabView = lazy(() =>
  import("@/app/conversation/FileTabView").then((module) => ({ default: module.FileTabView })),
);
const ReviewTab = lazy(() =>
  import("@/app/conversation/ReviewTab").then((module) => ({ default: module.ReviewTab })),
);

const BrowserTab = lazy(() => import("./BrowserTab").then((module) => ({ default: module.BrowserTab })));
const SideChatTab = lazy(() => import("./SideChatTab").then((module) => ({ default: module.SideChatTab })));
const MainTerminalTab = lazy(() => import("./MainTerminalTab").then((module) => ({ default: module.MainTerminalTab })));
const DocumentTabView = lazy(() => import("./DocumentTabView").then((module) => ({ default: module.DocumentTabView })));

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
            <Suspense fallback={null}>
              {tab.kind === "file" ? (
                <FileTabView conversationId={conversationId} tab={tab} active={shown} />
              ) : tab.kind === "review" ? (
                <ReviewTab conversationId={conversationId} target={tab.target} active={shown} />
              ) : tab.kind === "browser" ? (
                <BrowserTab conversationId={tab.id} initialUrl={tab.url} active={shown} />
              ) : tab.kind === "sideChat" ? (
                <SideChatTab conversationId={conversationId} tab={tab} />
              ) : tab.kind === "terminal" ? (
                <div data-main-terminal className="flex min-h-0 flex-1 flex-col"><MainTerminalTab conversationId={conversationId} tab={tab} active={shown} /></div>
              ) : <DocumentTabView conversationId={conversationId} tab={tab} active={shown} />}
            </Suspense>
          </div>
        );
      })}
    </>
  );
}
