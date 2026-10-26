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

import { TooltipIconButton, Badge } from "@/components/controls";
import { useCheckoutRoot } from "@/components/conversation";
import { UNTITLED } from "@/app/shell/TopBar";
import { Plus } from "@/components/icons";
import { sessionTabKey } from "@/state/sessionTabKeys";
import { shortcutLabel } from "@/app/side-panel/SidePanel";
import { takePaneClose } from "@/state/closedPanes";
import { undoTabClose } from "@/state/terminalPlaces";
import { BarDivider } from "@/app/shell/BarDivider";
import { ChatTab } from "@/app/shell/ChatTab";
import { SessionTab } from "@/app/shell/SessionTab";
import { OutputTab } from "@/app/side-panel/OutputTab";
import { useMockupTab } from "@/app/conversation/MockupTab";
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
  moveTab,
  type SessionTab as Tab,
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
import { NewTabTools } from "../side-panel/NewTabTools";

/** After a close, how long the pointer may be away from the tabs before they take the room back. */
const CLOSE_LOCK_MS = 300;

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
  const { listRef, shown, dragging, grip } = useDragReorder<Tab, HTMLDivElement>({
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

  // A close holds the tabs' widths only while they fill the bar: at their full width the next
  // tab's × slides under the pointer anyway, and "+" follows the last tab at once.
  const holdWidths = () => {
    if (full) lock.hold(tabs.length + 1);
  };
  // On the button's release: the webview may never send `auxclick`.
  const middleClose = (tab: string) => (event: ReactMouseEvent) => {
    if (event.button !== 1) return;
    event.preventDefault();
    holdWidths();
    closeTab(id, tab);
  };
  const newTab = (
    <TooltipIconButton look="toolbar" tooltip="New tab" shortcut={shortcutLabel("⌘T", mac)} className="ms-0.5"
      onClick={() => { lock.release(); create("newTab"); }}><Plus /></TooltipIconButton>
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
        <ChatTab
          conversation={conversation}
          title={chatTitle}
          active={active === CHAT_TAB}
          separator={active !== CHAT_TAB && tabs.length > 0 && tabs[0]?.id !== active || undefined}
          status={status}
          error={error}
          onRename={onRename}
        />
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
                <SessionTab
                  key={tab.id}
                  conversationId={id}
                  tab={tab}
                  name={name}
                  selected={selected}
                  separator={!selected && index < tabs.length - 1 && tabs[index + 1]?.id !== active || undefined}
                  closing={presence.closing.has(tab.id)}
                  dragging={dragging === tab.id}
                  handlers={handlers}
                  onMouseUp={middleClose(tab.id)}
                  onClose={() => {
                    holdWidths();
                    closeTab(id, tab.id);
                  }}
                />
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
function useTabPresence(tabs: readonly Tab[]) {
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
const file = preloadable(() => import("@/app/side-panel/FileTabView").then((module) => module.FileTabView));
const FileTabView = file.View;
const review = preloadable(() => import("@/app/side-panel/ReviewTab").then((module) => module.ReviewTab));
const ReviewTab = review.View;

/** Loads the file and Review tabs' code ahead of their first opening. */
export function preloadCodeTabs(): Promise<unknown> {
  return Promise.all([file.preload(), review.preload()]);
}

const BrowserTab = lazy(() => import("../side-panel/BrowserTab").then((module) => ({ default: module.BrowserTab })));
const SideChatTab = lazy(() => import("../side-panel/SideChatTab").then((module) => ({ default: module.SideChatTab })));
const MainTerminalTab = lazy(() => import("../side-panel/MainTerminalTab").then((module) => ({ default: module.MainTerminalTab })));
const MockupView = lazy(() => import("../side-panel/MockupTab").then((module) => ({ default: module.MockupView })));
const HistoryTab = lazy(() => import("../side-panel/HistoryTab").then((module) => ({ default: module.HistoryTab })));
const PullRequestHistory = lazy(() => import("../conversation/PullRequestHistory").then((module) => ({ default: module.PullRequestHistory })));
const SourcesTab = lazy(() => import("../side-panel/SourcesTab").then((module) => ({ default: module.SourcesTab })));
const PastedTextView = lazy(() => import("@/components/conversation/elements/pasted-text").then((module) => ({ default: module.PastedTextView })));
const ConflictTab = lazy(() => import("../side-panel/ConflictTab").then((module) => ({ default: module.ConflictTab })));
const PageTabView = lazy(() => import("../side-panel/pages/PageTabView").then((module) => ({ default: module.PageTabView })));
const TabWithChat = lazy(() => import("../side-panel/TabChat").then((module) => ({ default: module.TabWithChat })));

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
            className={cn("bg-background absolute inset-0 flex flex-col", !shown && "behind [&_*]:transition-none!")}
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
                : tab.kind === "pasted" ? <PastedTextView n={tab.n} paste={tab.paste} />
                : tab.kind === "panel" ? tab.view.type === "output" ? <OutputTab conversationId={conversationId} view={tab.view} />
                  : tab.view.type === "backgroundHistory" ? <HistoryTab conversationId={conversationId} />
                  : tab.view.type === "pr-history" ? <PullRequestHistory sessionId={conversationId} />
                  : <SourcesTab conversationId={conversationId} />
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
