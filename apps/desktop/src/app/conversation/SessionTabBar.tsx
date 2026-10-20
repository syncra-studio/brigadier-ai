import {
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type CSSProperties,
  useState,
  useCallback,
  lazy,
  Suspense,
  useEffect,
  useLayoutEffect,
  useRef,
} from "react";

import { useCheckoutRoot } from "@/components/assistant-ui/file-links";
import { TitlebarButton } from "@/components/titlebar-button";
import { SidebarActivityMark, SidebarError, statusWords } from "@/app/SessionStatus";
import { UNTITLED } from "@/app/TopBar";
import { Archive, Chat, ChatCompose, Dock, FileDocument, Globe, Lightbulb, Plus, Document, ShieldCheck, Terminal, X } from "@/components/icons";
import { sessionTabKey } from "@/state/sessionTabKeys";
import { shortcutLabel } from "@/app/conversation/SidePanel";
import { takePaneClose } from "@/state/closedPanes";
import { undoTabClose } from "@/state/terminalPlaces";
import { ChatActions } from "@/app/conversation/ChatActions";
import { OutputTab } from "@/app/conversation/OutputTab";
import { MockupTabIcon, MockupTabStatus, useMockupTab } from "@/app/conversation/MockupTabIcon";
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
import { preloadable } from "@/lib/preloadable";
import { cn } from "@/lib/utils";
import { closeSession } from "@/state/actions";
import {
  CHAT_TAB,
  newSessionTab,
  replaceNewTab,
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
} from "@/state/sessionTabs";
import { useBrowserOn } from "@/state/links";
import { tabTitle } from "@/state/sessionTabTitle";
import { useApp } from "@/state/store";
import { useSessionStatus, useSidebarIndicators } from "@/state/tabStates";
import { NewTabTools } from "./NewTabTools";
import { BrowserTabIcon } from "./BrowserTabIcon";

/** Tabs share the available row, then scroll once they reach their minimum width. */
const TAB =
  "session-tab group/tab relative flex h-control-md min-w-0 items-center gap-1 rounded-session-tab ps-2.5 pe-1.75 select-none";

/** After a close, how long the pointer may be away from the tabs before they take the room back. */
const CLOSE_LOCK_MS = 300;

/** Mouse down with the middle button would start autoscroll. */
function noAutoscroll(event: ReactMouseEvent): void {
  if (event.button === 1) event.preventDefault();
}

/** Session keys own main tabs. The bottom pane keeps Cmd+W and bracket navigation; off macOS a focused
 * terminal keeps its shell Ctrl keys. Cmd+T always creates a main tab on macOS. An archived session
 * opens no terminal tabs, so its Ctrl+` goes on to the bottom-terminal toggle. */
function useTabKeys(conversationId: string, archived: boolean, create: (kind: NewTabKind) => void): void {
  const mac = useApp((s) => s.info?.platform === "macos");
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return;
      const target = event.target instanceof Element ? event.target : document.activeElement;
      if (!mac && target?.closest('[data-slot="terminal-pane"], [data-main-terminal]')) return;
      // ⌥⌘W is the session's own, even there.
      if (!event.altKey && target?.closest('[data-slot="terminal-pane"]') && ["KeyW", "BracketLeft", "BracketRight"].includes(event.code)) return;
      const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (command && !event.altKey && event.shiftKey && event.code === "KeyT") {
        event.preventDefault(); event.stopPropagation();
        const closed = takePaneClose(conversationId);
        if (closed === "tab") reopenTab(conversationId, useApp.getState().conversations[conversationId]?.lifecycle !== "archived");
        else if (closed === "terminal") undoTabClose(`conv:${conversationId}`);
        return;
      }
      const action = sessionTabKey(event, mac);
      if (!action || (archived && action.type === "new" && action.kind === "terminal")) return;
      event.preventDefault(); event.stopPropagation();
      if (action.type === "new") create(action.kind);
      if (action.type === "close" && sessionTabs(conversationId).active === CHAT_TAB) closeSession(conversationId);
      else if (action.type === "close") closeTab(conversationId, sessionTabs(conversationId).active);
      if (action.type === "closeOthers") closeOtherTabs(conversationId, sessionTabs(conversationId).active);
      if (action.type === "number") selectTabNumber(conversationId, action.number);
      if (action.type === "step") stepTab(conversationId, action.step);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [conversationId, archived, mac, create]);
}

/** A tab's right-click menu: Keep open (a preview), Close, Close others, Close to the right. */
function TabMenu({
  conversationId,
  tab,
  onRename,
  children,
}: {
  conversationId: string;
  tab: SessionTab | null;
  onRename?: (() => void) | undefined;
  children: ReactNode;
}) {
  const id = tab?.id ?? CHAT_TAB;
  const order = tabOrder(sessionTabs(conversationId));
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent>
        {!tab && (
          <>
            <ContextMenuItem disabled={!onRename} onSelect={() => onRename?.()}>Rename</ContextMenuItem>
            <ContextMenuSeparator />
          </>
        )}
        {tab?.kind === "file" && tab.preview && (
          <>
            <ContextMenuItem onSelect={() => keepTabOpen(conversationId, id)}>
              Keep open
            </ContextMenuItem>
            <ContextMenuSeparator />
          </>
        )}
        <ContextMenuItem onSelect={() => (tab ? closeTab(conversationId, id) : closeSession(conversationId))}>
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

/** The rule between the bar's own controls and the toolbar's; `hidden` fades it but keeps its place. */
function BarDivider({ hidden = false }: { hidden?: boolean }) {
  return (
    <span aria-hidden data-bar-divider data-hidden={hidden || undefined}
      className="bg-session-tab-separator mx-0.5 h-4 w-px shrink-0 transition-opacity duration-150 data-hidden:opacity-0 motion-reduce:transition-none" />
  );
}

/**
 * A session's top bar: always tabs, the chat first under its title with its ⋯ (never moved; its ×
 * closes the session's view, which goes on working), even when it is the only one, then the tabs
 * it opened, which shrink to their least and then scroll, and "+" right after them.
 * Click to show, drag to reorder, middle-click or Delete to close; a preview tab is in italic
 * until double-clicked. After a close the tabs keep their widths, so the next × lands under the
 * pointer, until it leaves them. While the thread keeps something running for the user, its chip
 * ("web · Running · Stop") follows. `children` sit at the end: the conversation's toolbar.
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
  const { tabs, active } = useSessionTabsOf(id);
  useMockupTab(id);
  const connection = useApp((s) => s.connection.status);
  const archived = conversation.lifecycle === "archived";
  const status = useSessionStatus(id);
  const { error } = useSidebarIndicators(id);
  const strip = useRef<HTMLDivElement>(null);
  const mac = useApp((s) => s.info?.platform === "macos");
  const cwd = useCheckoutRoot();
  const browserOn = useBrowserOn();
  const create = useCallback((kind: NewTabKind) => {
    if (archived && (kind === "terminal" || kind === "sideChat")) return;
    // Off, the built-in browser doesn't exist: ⇧⌘B and its menu item do nothing.
    if (kind === "browser" && !browserOn) return;
    newSessionTab(id, kind, cwd);
  }, [id, archived, cwd, browserOn]);
  useTabKeys(id, archived, create);
  const presence = useTabPresence(tabs);
  const lock = useCloseLock(presence.tabs.length === 0);
  const { listRef, shown, dragging, grip } = useDragReorder<SessionTab, HTMLDivElement>({
    items: presence.tabs,
    idOf: (tab) => tab.id,
    rowSelector: "[data-session-tab]",
    onMove: (tab, to) => moveTab(id, tab, to),
    axis: "x",
  });

  // The tab in front scrolls into view, and stays there as the strip's room changes and as the
  // tabs finish growing or shrinking.
  const tabCount = tabs.length;
  useEffect(() => {
    const list = strip.current;
    if (!list || tabCount === 0) return;
    const reveal = () =>
      list
        .querySelector<HTMLElement>(`[data-tab-id="${CSS.escape(active)}"]`)
        ?.scrollIntoView({ block: "nearest", inline: "nearest" });
    reveal();
    const observer = new ResizeObserver(reveal);
    observer.observe(list);
    list.addEventListener("transitionend", reveal);
    return () => {
      observer.disconnect();
      list.removeEventListener("transitionend", reveal);
    };
  }, [active, tabCount]);

  // The rule before the toolbar shows once the tabs fill the bar, when no room is left after "+".
  const [gap, setGap] = useState<HTMLDivElement | null>(null);
  const [full, setFull] = useState(false);
  useLayoutEffect(() => {
    if (!gap) return;
    const measure = () => setFull(gap.getBoundingClientRect().width < 1);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(gap);
    return () => observer.disconnect();
  }, [gap]);

  // On the button's release: the webview may never send `auxclick`.
  const middleClose = (tab: string) => (event: ReactMouseEvent) => {
    if (event.button !== 1) return;
    event.preventDefault();
    lock.hold(tabs.length + 1);
    closeTab(id, tab);
  };
  const newTab = (
    <TitlebarButton tooltip="New tab" shortcut={shortcutLabel("⌘T", mac)} className="ms-0.5"
      onClick={() => { lock.release(); create("newTab"); }}><Plus /></TitlebarButton>
  );
  const connecting = connection !== "connected" && (
    <Badge variant="warning" role="status" className="shrink-0">
      {connection === "connecting" ? "Connecting to core…" : "Reconnecting to core…"}
    </Badge>
  );
  const chatTitle = UNTITLED.has(conversation.title) ? "Chat" : conversation.title;

  return (
    <header
      data-tauri-drag-region
      data-slot="session-tabs"
      className="column-divider h-titlebar ease-sidebar ps-clear-2 flex shrink-0 items-center gap-1 pe-1 transition-[padding] duration-300 motion-reduce:transition-none"
    >
      <div
        role="tablist"
        aria-label="Session tabs"
        tabIndex={-1}
        data-tauri-drag-region
        className="session-tab-list flex min-w-0 items-center gap-0.5"
        style={{ "--session-tab-count": lock.count ?? tabs.length + 1 } as CSSProperties}
        onPointerEnter={lock.stay}
        onPointerLeave={lock.leave}
        onKeyDown={(event) => {
          if (
            (event.key === "Backspace" || event.key === "Delete") &&
            !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey &&
            event.target instanceof HTMLElement && event.target.getAttribute("role") === "tab"
          ) {
            // Delete closes the tab that has the keyboard (never Chat), then the keyboard goes
            // to the tab in front.
            const tab = event.target.closest<HTMLElement>("[data-tab-id]")?.dataset.tabId;
            if (!tab || tab === CHAT_TAB) return;
            event.preventDefault();
            closeTab(id, tab);
            // The tab now in front was already in the strip.
            const front = sessionTabs(id).active;
            event.currentTarget.querySelector<HTMLElement>(`[data-tab-id="${CSS.escape(front)}"] [role="tab"]`)?.focus();
            return;
          }
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
        <TabMenu conversationId={id} tab={null} onRename={archived ? undefined : onRename}>
          <div
            data-tab-id={CHAT_TAB}
            data-separator={active !== CHAT_TAB && tabs.length > 0 && tabs[0]?.id !== active || undefined}
            data-active={active === CHAT_TAB || undefined}
            className={cn(
              TAB,
              active === CHAT_TAB
                ? "bg-panel-tab shadow-session-tab text-foreground"
                : "text-toolbar-foreground hover:bg-toolbar-hover",
            )}
          >
            <button
              type="button"
              role="tab"
              aria-selected={active === CHAT_TAB}
              tabIndex={active === CHAT_TAB ? 0 : -1}
              title={archived ? `${chatTitle} (Archived)` : status ? `${chatTitle} · ${statusWords(status)}` : chatTitle}
              aria-label={archived ? `${chatTitle} (Archived)` : undefined}
              onMouseDown={noAutoscroll}
              onMouseUp={(event) => { if (event.button === 1) { event.preventDefault(); closeSession(id); } }}
              onClick={() => selectTab(id, CHAT_TAB)}
              onDoubleClick={archived ? undefined : onRename}
              className="flex h-full min-w-0 flex-1 items-center gap-1 rounded-sm"
            >
              {/* The sidebar row's own marks, without its state words: its error glyph leads, its spinner trails. */}
              {archived ? <Archive aria-hidden className="size-icon-sm shrink-0" /> : error ? <SidebarError conversationId={id} /> : <Chat aria-hidden className="size-icon-sm shrink-0" />}
              <span className="session-tab-title min-w-0 flex-1 overflow-hidden text-start whitespace-nowrap">{chatTitle}</span>
              {/* The session in front is open: it is never unread. */}
              {!archived && <SidebarActivityMark conversationId={id} unread={false} />}
            </button>
            <ChatActions
              conversation={conversation}
              onRename={onRename}
              compact
              className="shrink-0"
            />
            {/* Closes the view only: the session goes on working and stays in the sidebar. */}
            <button
              type="button"
              aria-label={`Close ${chatTitle} tab`}
              onClick={(event) => { event.stopPropagation(); closeSession(id); }}
              className={cn(
                "hover:bg-toolbar-hover flex size-5 shrink-0 items-center justify-center rounded-full opacity-60 hover:opacity-100",
                active !== CHAT_TAB && "invisible group-focus-within/tab:visible group-hover/tab:visible",
              )}
            >
              <X aria-hidden className="size-icon-xs" />
            </button>
          </div>
        </TabMenu>
        <div
          ref={strip}
          data-tauri-drag-region
          className="hide-scrollbar flex min-w-0 flex-1 scroll-px-1 items-center gap-0.5 overflow-x-auto"
        >
          <div ref={listRef} className="contents">
            {shown.map((tab) => {
              const index = tabs.findIndex((entry) => entry.id === tab.id);
              const selected = tab.id === active;
              const name = tabTitle(tab, tabs);
              const handlers = grip(tab.id, index);
              return (
                <TabMenu key={tab.id} conversationId={id} tab={tab}>
                  <AnimatedTab
                    data-session-tab
                    closing={presence.closing.has(tab.id)}
                    inert={presence.closing.has(tab.id)}
                    aria-hidden={presence.closing.has(tab.id) || undefined}
                    data-separator={!selected && index < tabs.length - 1 && tabs[index + 1]?.id !== active || undefined}
                    data-tab-id={tab.id}
                    data-active={selected || undefined}
                    data-dragging={dragging === tab.id || undefined}
                    className={cn(
                      TAB,
                      selected
                        ? "bg-panel-tab shadow-session-tab text-foreground"
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
                      title={tab.kind === "file" ? tab.path : tab.kind === "sideChat" && tab.title ? `Side chat · ${tab.title}` : name}
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
                      className="flex h-full min-w-0 flex-1 items-center gap-1 rounded-sm"
                    >
                      {tab.kind === "file" || tab.kind === "conflict" ? (
                        <FileTypeIcon name={tab.path} className="size-icon-sm shrink-0" />
                      ) : tab.kind === "newTab" ? <Globe className="size-icon-sm shrink-0" />
                        : tab.kind === "browser" ? <BrowserTabIcon tabId={tab.id} />
                        : tab.kind === "terminal" ? <Dock className="size-icon-sm shrink-0" />
                        : tab.kind === "sideChat" ? <ChatCompose className="size-icon-sm shrink-0" />
                        : tab.kind === "page" ? <Document className="size-icon-sm shrink-0" />
                        : tab.kind === "mockup" ? <MockupTabIcon conversationId={id} />
                        : tab.kind === "plan" ? <Lightbulb className="size-icon-sm shrink-0" />
                        : tab.kind === "pasted" ? <FileDocument className="size-icon-sm shrink-0" />
                        : tab.kind === "panel" ? tab.view.type === "output" ? <Terminal className="size-icon-sm shrink-0" /> : tab.view.type === "sources" ? <Globe className="size-icon-sm shrink-0" /> : <ShieldCheck className="size-icon-sm shrink-0" /> : (
                        <DiffGlyph className="size-icon-sm shrink-0" />
                      )}
                      <span className={cn("session-tab-title min-w-0 flex-1 overflow-hidden text-start whitespace-nowrap", tab.kind === "file" && tab.preview && "italic", tab.kind === "conflict" && tab.view !== "compare" && "text-destructive")}>
                        {name}
                        {tab.kind === "mockup" && <MockupTabStatus conversationId={id} />}
                      </span>
                      {tab.unseen && <span data-slot="tab-unseen" role="img" aria-label="New" className="bg-link size-2 shrink-0 rounded-full" />}
                    </button>
                    <button
                      type="button"
                      aria-label={`Close ${name} tab`}
                      onClick={(event) => {
                        event.stopPropagation();
                        lock.hold(tabs.length + 1);
                        closeTab(id, tab.id);
                      }}
                      className={cn(
                        "hover:bg-toolbar-hover flex size-5 shrink-0 items-center justify-center rounded-full opacity-60 hover:opacity-100",
                        !selected && "invisible group-focus-within/tab:visible group-hover/tab:visible",
                      )}
                    >
                      <X aria-hidden className="size-icon-xs" />
                    </button>
                  </AnimatedTab>
                </TabMenu>
              );
            })}
          </div>
        </div>
      </div>
      {newTab}
      {connecting}
      <div ref={setGap} data-tauri-drag-region className="min-w-0 flex-1 self-stretch" />
      <BarDivider hidden={!full} />
      {children}
    </header>
  );
}

/** Keep removed tabs only for their closing slide; state and resources close immediately. */
function useTabPresence(tabs: readonly SessionTab[]) {
  const [state, setState] = useState({ source: tabs, tabs, closing: new Set<string>() });
  if (state.source !== tabs) {
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const shown = [...tabs];
    const closing = new Set<string>();
    if (!reduced) {
      state.tabs.forEach((tab, index) => {
        if (!tabs.some((current) => current.id === tab.id)) {
          shown.splice(Math.min(index, shown.length), 0, tab);
          closing.add(tab.id);
        }
      });
    }
    setState({ source: tabs, tabs: shown, closing });
  }
  useEffect(() => {
    if (!state.closing.size) return;
    const timer = setTimeout(() => setState((current) => ({
      source: current.source, tabs: current.source, closing: new Set<string>(),
    })), 100);
    return () => clearTimeout(timer);
  }, [state]);
  return state;
}

/** A new tab grows into its slot; the final width still comes from the responsive CSS rule. */
function AnimatedTab({ closing, ref: triggerRef, ...props }: React.ComponentProps<"div"> & { closing: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const attach = useCallback((element: HTMLDivElement | null) => {
    ref.current = element;
    if (typeof triggerRef === "function") return triggerRef(element);
    if (triggerRef) triggerRef.current = element;
  }, [triggerRef]);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element || closing || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const style = getComputedStyle(element);
    const animation = element.animate([
      { flexBasis: "0", paddingInlineStart: "0", paddingInlineEnd: "0", opacity: 0, overflow: "hidden" },
      { flexBasis: `${element.getBoundingClientRect().width}px`, paddingInlineStart: style.paddingInlineStart, paddingInlineEnd: style.paddingInlineEnd, opacity: 1, overflow: "hidden" },
    ], { duration: 200, easing: style.getPropertyValue("--ease-session-tab").trim() });
    return () => animation.cancel();
  }, [closing]);
  return <div ref={attach} data-closing={closing || undefined} {...props} />;
}

/**
 * Keeps the tabs' widths after a close (their count as it was), so the next tab's × comes under
 * the pointer. Let go once the pointer has been away from the tabs a moment, on a key or a
 * scroll, when the window loses focus, or when no tab is left.
 */
function useCloseLock(alone: boolean) {
  const [count, setCount] = useState<number | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const release = useCallback(() => {
    clearTimeout(timer.current);
    setCount(null);
  }, []);
  const locked = count !== null;
  useEffect(() => {
    if (!locked) return;
    window.addEventListener("keydown", release, true);
    window.addEventListener("wheel", release, true);
    window.addEventListener("blur", release);
    return () => {
      window.removeEventListener("keydown", release, true);
      window.removeEventListener("wheel", release, true);
      window.removeEventListener("blur", release);
    };
  }, [locked, release]);
  useEffect(() => () => clearTimeout(timer.current), []);
  if (alone && locked) setCount(null);
  return {
    count,
    release,
    hold: (current: number) => setCount((held) => held ?? current),
    stay: () => clearTimeout(timer.current),
    leave: () => {
      if (!locked) return;
      clearTimeout(timer.current);
      timer.current = setTimeout(release, CLOSE_LOCK_MS);
    },
  };
}

// Warmed at idle, so a file from the tree and View changes show on the first frame
// (`preloadCodeTabs`): a lazy view would hold its blank fallback for a moment more.
const file = preloadable(() => import("@/app/conversation/FileTabView").then((module) => module.FileTabView));
const FileTabView = file.View;
const review = preloadable(() => import("@/app/conversation/ReviewTab").then((module) => module.ReviewTab));
const ReviewTab = review.View;

/** Loads the file and Review tabs' code ahead of their first opening. */
export function preloadCodeTabs(): Promise<unknown> {
  return Promise.all([file.preload(), review.preload()]);
}

const BrowserTab = lazy(() => import("./BrowserTab").then((module) => ({ default: module.BrowserTab })));
const SideChatTab = lazy(() => import("./SideChatTab").then((module) => ({ default: module.SideChatTab })));
const MainTerminalTab = lazy(() => import("./MainTerminalTab").then((module) => ({ default: module.MainTerminalTab })));
const MockupView = lazy(() => import("./MockupTab").then((module) => ({ default: module.MockupView })));
const SessionJobTab = lazy(() => import("./JobTab").then((module) => ({ default: module.SessionJobTab })));
const SourcesTab = lazy(() => import("./SourcesTab").then((module) => ({ default: module.SourcesTab })));
const PlanTab = lazy(() => import("./PlanTab").then((module) => ({ default: module.PlanTab })));
const PastedTextView = lazy(() => import("@/components/assistant-ui/elements/pasted-text").then((module) => ({ default: module.PastedTextView })));
const ConflictTab = lazy(() => import("./ConflictTab").then((module) => ({ default: module.ConflictTab })));
const PageTabView = lazy(() => import("./pages/PageTabView").then((module) => ({ default: module.PageTabView })));
const TabWithChat = lazy(() => import("./TabChat").then((module) => ({ default: module.TabWithChat })));

/**
 * The session's open tabs over its conversation, which stays as it was (scrolled, its draft
 * typed) under them: each tab keeps its place too, only the one in front shows. Nothing shows
 * while Chat is in front.
 */
export function SessionTabViews({ conversationId }: { conversationId: string }) {
  const { tabs, active } = useSessionTabsOf(conversationId);
  const browserOn = useBrowserOn();
  return (
    <>
      {tabs.map((tab) => {
        const shown = tab.id === active;
        return (
          <div
            key={tab.id}
            role="tabpanel"
            aria-label={tabTitle(tab, tabs)}
            inert={!shown}
            // Behind, nothing in it transitions: a button's `transition-all` would keep it
            // visible over the thread a moment longer (#417).
            className={cn("bg-background absolute inset-0 flex flex-col", !shown && "invisible [&_*]:transition-none!")}
          >
            <Suspense fallback={null}>
              {tab.kind === "file" ? (
                <TabWithChat conversationId={conversationId} tab={tab}>
                  <FileTabView conversationId={conversationId} tab={tab} active={shown} />
                </TabWithChat>
              ) : tab.kind === "review" ? (
                <ReviewTab conversationId={conversationId} target={tab.target} active={shown} />
              ) : tab.kind === "newTab" && !browserOn ? (
                // Off, a New tab offers its tools without an address bar.
                <div className="@container/new-tab flex min-h-0 flex-1 flex-col">
                  <NewTabTools conversationId={conversationId} tabId={tab.id} />
                </div>
              ) : tab.kind === "newTab" ? (
                <BrowserTab conversationId={tab.id} active={shown}
                  onNavigate={(url) => replaceNewTab(conversationId, tab.id, "browser", null, url)}>
                  <NewTabTools conversationId={conversationId} tabId={tab.id} />
                </BrowserTab>
              ) : tab.kind === "browser" ? (
                <TabWithChat conversationId={conversationId} tab={tab}>
                  <BrowserTab conversationId={tab.id} initialUrl={tab.url} agent={tab.agent} active={shown} />
                </TabWithChat>
              ) : tab.kind === "sideChat" ? (
                <SideChatTab conversationId={conversationId} tab={tab} />
              ) : tab.kind === "terminal" ? (
                <div data-main-terminal className="flex min-h-0 flex-1 flex-col"><MainTerminalTab conversationId={conversationId} tab={tab} active={shown} /></div>
              ) : tab.kind === "mockup" ? <MockupView conversationId={conversationId} active={shown} />
                : tab.kind === "plan" ? <PlanTab plan={tab.plan} />
                : tab.kind === "pasted" ? <PastedTextView n={tab.n} paste={tab.paste} />
                : tab.kind === "panel" ? tab.view.type === "output" ? <OutputTab conversationId={conversationId} view={tab.view} />
                  : tab.view.type === "sources" ? <SourcesTab conversationId={conversationId} /> : <SessionJobTab conversationId={conversationId} />
                : tab.kind === "conflict" ? <ConflictTab conversationId={conversationId} tab={tab} />
                : (
                  <TabWithChat conversationId={conversationId} tab={tab}>
                    <PageTabView conversationId={conversationId} tab={tab} active={shown} />
                  </TabWithChat>
                )}
            </Suspense>
          </div>
        );
      })}
    </>
  );
}
