import {
  ActionBarPrimitive,
  AuiIf,
  type AssistantState,
  BranchPickerPrimitive,
  ComposerPrimitive,
  MessagePrimitive,
  TextMessagePartProvider,
  type TextMessagePartProps,
  ThreadPrimitive,
  useAui,
  useAuiEvent,
  useAuiState,
} from "@assistant-ui/react";
import {
  defaultRangeExtractor,
  measureElement as measuredSize,
  observeElementOffset,
  observeElementRect,
  type Range as VirtualRange,
  useVirtualizer,
} from "@tanstack/react-virtual";
import {
  createContext,
  memo,
  Suspense,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ComponentType,
  type FC,
  type FocusEvent as ReactFocusEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";

import { useFileDrops } from "@/app/conversation/fileDrops";
import { CopyButton } from "@/components/shared/copy-button";
import { LinkTimeContext } from "@/components/conversation/file-links";
import { MessageRail, ThreadHistoryContext } from "@/components/conversation/message-rail";
import { RowScope, rowKeyOf, useKeptState } from "@/components/conversation/row-memory";
import { keptPlace, ThreadScroller } from "@/components/conversation/thread-scroll";
import { useThreadList, useThreadListIds, useThreadListSource } from "@/components/conversation/thread-list";
import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { TurnBoundary } from "@/components/conversation/turn-boundary";
import { Button } from "@/components/controls/button";
import {
  ArrowDown,
  ChevronLeft,
  ChevronRight,
  ChevronRightAlt,
  EditPencil,
  Paperclip,
} from "@/components/icons";
import { ScrollFade } from "@/components/layout/scroll-fade";
import { Skeleton } from "@/components/layout/skeleton";
import { markdownText } from "@/components/conversation/markdown-lazy";
import { useNow } from "@/hooks/use-now";
import { formatDaySeparator, formatSentAt, separatesTurn } from "@/lib/format";
import { cn } from "@/lib/utils";

/**
 * `AssistantMessage`, `Welcome` and `Composer` are whole sections the app supplies;
 * `BeforeMessages` renders above the message list (e.g. a "load earlier" control);
 * `UserAttachments` above a user message's text (its files);
 * `AboveComposer` renders between the messages and the composer (queue, notices).
 */
export type ThreadComponents = {
  AssistantMessage: ComponentType;
  Welcome: ComponentType;
  BeforeMessages?: ComponentType | undefined;
  UserAttachments?: ComponentType | undefined;
  AboveComposer?: ComponentType | undefined;
  /** A message another session sent, in the user message's place (its `custom.peer` is set). */
  PeerMessage?: ComponentType | undefined;
  /** A user message's text, where the app shows what it mentions. */
  UserText?: ComponentType<TextMessagePartProps> | undefined;
  /** Floats centred just above the composer, over the thread (the capsule). */
  Capsule?: ComponentType | undefined;
  Composer: ComponentType<ComposerProps>;
  /** Over the thread's top end: the app's find bar (⌘F). */
  Find?: ComponentType<ThreadOverlayProps> | undefined;
};

/** What an overlay of the thread works with: its scroll element and its scroll controller. */
export type ThreadOverlayProps = { viewport: HTMLDivElement | null; scroller: ThreadScroller };

export type ComposerProps = { autoFocus: boolean; placeholder: string };

export type ThreadProps = {
  components: ThreadComponents;
  autoFocus?: boolean | undefined;
  placeholder?: string | undefined;
  /** Where the thread was scrolled is kept under this key (the conversation) while it is away. */
  scrollKey?: string | undefined;
};

const ThreadComponentsContext = createContext<ThreadComponents | null>(null);

function useThreadComponents(): ThreadComponents {
  const components = useContext(ThreadComponentsContext);
  if (!components) throw new Error("useThreadComponents is used outside a Thread");
  return components;
}

// Startup exposes a loading placeholder thread; treat it as a new chat so
// the composer mounts centered. Loads after startup keep the docked layout.
const isNewChatView = (s: AssistantState) =>
  s.thread.messages.length === 0 &&
  (!s.thread.isLoading || s.threads.isLoading);

// A switched thread that is still fetching its history: an empty column, not the welcome.
const isHistoryLoadingView = (s: AssistantState) =>
  s.thread.messages.length === 0 &&
  s.thread.isLoading &&
  !s.thread.isDisabled &&
  !s.threads.isLoading;

/** While a thread's history loads its column stays empty, with no placeholder lines; only a screen reader hears it. */
const ThreadHistoryLoading: FC = () => (
  <span data-slot="aui_thread-history-loading" role="status" className="sr-only">
    Loading conversation
  </span>
);

// Drawn again only when its own props or what it reads change: not with the view around it, which
// draws on every streamed word.
export const Thread = memo<ThreadProps>(function Thread({
  components,
  autoFocus = true,
  placeholder = "Send a message...",
  scrollKey,
}) {
  const isEmpty = useAuiState(isNewChatView);

  return (
    <ThreadComponentsContext.Provider value={components}>
      <ThreadRoot
        isEmpty={isEmpty}
        autoFocus={autoFocus}
        placeholder={placeholder}
        scrollKey={scrollKey ?? null}
      />
    </ThreadComponentsContext.Provider>
  );
});

const ThreadScrollContext = createContext<ThreadScroller | null>(null);
/** The key the thread keeps its place and its messages' parts under (the conversation). */
const ThreadKeyContext = createContext<string | null>(null);

/** The key the thread around keeps its messages' parts under. */
export function useThreadKey(): string | null {
  return useContext(ThreadKeyContext);
}

/**
 * Wires the scroll element, its column, the message rows, the room after them, the pad under
 * them and the floating footer to the thread's scroll controller (see `thread-scroll.ts`).
 */
function useThreadScroll(
  scroller: ThreadScroller,
  saveKey: string | null,
): {
  viewport: HTMLDivElement | null;
  group: HTMLDivElement | null;
  setViewport: (element: HTMLDivElement | null) => void;
  setColumn: (element: HTMLDivElement | null) => void;
  setGroup: (element: HTMLDivElement | null) => void;
  setSpacer: (element: HTMLDivElement | null) => void;
  setPad: (element: HTMLDivElement | null) => void;
  setFooter: (element: HTMLDivElement | null) => void;
} {
  const [viewport, setViewport] = useState<HTMLDivElement | null>(null);
  const [column, setColumn] = useState<HTMLDivElement | null>(null);
  const [group, setGroup] = useState<HTMLDivElement | null>(null);
  const [spacer, setSpacer] = useState<HTMLDivElement | null>(null);
  const [pad, setPad] = useState<HTMLDivElement | null>(null);
  const [footer, setFooter] = useState<HTMLDivElement | null>(null);

  // A sent turn's room lasts while the thread works on it. Told before attaching, so a
  // conversation shown again knows whether its kept room's turn still runs.
  const running = useAuiState((s) => s.thread.isRunning);
  useLayoutEffect(() => {
    scroller.setRunning(running);
  }, [scroller, running]);

  useLayoutEffect(() => {
    if (!viewport || !column || !group || !spacer || !pad || !footer) return;
    // The message roots, in the rows the list mounts (the spacers between them left out); read
    // live, as a row's message may be drawn afresh in place (edited).
    const list = () =>
      [...group.children].flatMap((child) =>
        child instanceof HTMLElement && child.dataset["threadRow"] !== undefined && child.firstElementChild instanceof HTMLElement
          ? [child.firstElementChild]
          : [],
      );
    // The pad and the scroll padding follow the floating footer's height (see globals.css).
    const footerResized = () =>
      viewport.style.setProperty("--thread-footer-height", `${footer.offsetHeight}px`);
    footerResized();
    const detach = scroller.attach(
      {
        viewport,
        rows: list,
        content: () => spacer.getBoundingClientRect().bottom - column.getBoundingClientRect().top,
        padding: () => pad.offsetHeight,
        footer: () => footer.offsetHeight,
        setRoom: (px) => {
          spacer.style.height = `${px}px`;
        },
      },
      saveKey,
    );

    // The view, the floating footer and what the column holds above the room (the rows, and a
    // skeleton or "earlier messages" before them): never the room or the pad, which change
    // here in answer.
    const resizes = new ResizeObserver((entries) => {
      if (entries.some((entry) => entry.target === footer)) footerResized();
      scroller.resized();
    });
    const observed = new Set<Element>();
    const observe = () => {
      const now = new Set([viewport, footer, ...[...column.children].filter((child) => child !== spacer && !child.contains(pad))]);
      for (const element of observed) if (!now.has(element)) resizes.unobserve(element);
      for (const element of now) if (!observed.has(element)) resizes.observe(element);
      observed.clear();
      for (const element of now) observed.add(element);
    };
    observe();
    const above = new MutationObserver(observe);
    above.observe(column, { childList: true });
    const children = new MutationObserver(() => scroller.rowsChanged());
    children.observe(group, { childList: true });

    const listening = new AbortController();
    const options = { passive: true, signal: listening.signal };
    // Heard before the message list hears it (a capturing listener on the view runs first): the
    // rows it mounts for where the view went change what is above, kept from where the view is.
    viewport.addEventListener("scroll", () => scroller.scrolled(), { ...options, capture: true });
    for (const type of ["wheel", "touchstart", "touchmove", "touchend", "pointerdown", "keydown"]) {
      viewport.addEventListener(type, scroller.input, options);
    }
    // A focus moved in from outside (Tab from the composer, the latest response's pill) scrolls to it.
    viewport.addEventListener("focusin", scroller.focused, options);
    // A pointer pressed in the thread may be let go anywhere.
    for (const type of ["pointerup", "pointercancel"]) window.addEventListener(type, scroller.input, options);
    // A click on the list's empty space focuses it but leaves the caret in the composer, where
    // Cmd+Up and Cmd+Down would move the caret rather than scroll the list.
    viewport.addEventListener(
      "focus",
      () => {
        const selection = getSelection();
        const anchor = selection?.anchorNode;
        const element = anchor instanceof Element ? anchor : anchor?.parentElement;
        if (element?.closest("[contenteditable]")) selection?.removeAllRanges();
      },
      options,
    );
    // Capturing: the toggle's place is taken before its fold changes.
    viewport.addEventListener(
      "click",
      (event) => {
        // An icon's SVG, too: its button is found from it.
        if (event.target instanceof Element) scroller.clicked(event.target);
      },
      { ...options, capture: true },
    );

    return () => {
      listening.abort();
      children.disconnect();
      above.disconnect();
      resizes.disconnect();
      detach();
    };
  }, [scroller, saveKey, viewport, column, group, spacer, pad, footer]);

  // A sent edit's new message is placed like a new one (new messages are placed by their id,
  // see `expectSentTurn`).
  useAuiEvent({ scope: "thread", event: "composer.send" }, (event) => {
    if (event.messageId) scroller.editSent();
  });

  return { viewport, group, setViewport, setColumn, setGroup, setSpacer, setPad, setFooter };
}

/**
 * "Drop to attach": a soft accent tint over the thread and a small pill, while files are dragged
 * onto it (`native`: by the window's own drag, which the dropzone doesn't see).
 */
const DropOverlay: FC<{ native: boolean }> = ({ native }) => (
  <div
    aria-hidden
    data-slot="drop-overlay"
    className={cn(
      "bg-link/8 animate-in fade-in-0 pointer-events-none absolute inset-0 z-30 hidden items-center justify-center duration-150 group-data-[dragging=true]/drop:flex",
      native && "flex",
    )}
  >
    <div
      data-slot="drop-pill"
      className="bg-popover text-popover-foreground rounded-capsule flex items-center gap-1.5 px-3 py-1.5 text-sm shadow-sm"
    >
      <Paperclip className="text-muted-foreground size-icon-sm" />
      Drop to attach
    </div>
  </div>
);

const ThreadRoot: FC<{
  isEmpty: boolean;
  autoFocus: boolean;
  placeholder: string;
  scrollKey: string | null;
}> = ({ isEmpty, autoFocus, placeholder, scrollKey }) => {
  const { Welcome, BeforeMessages, AboveComposer, Capsule, Composer, Find } = useThreadComponents();
  const [scroller] = useState(() => new ThreadScroller());
  const { viewport, group, setViewport, setColumn, setGroup, setSpacer, setPad, setFooter } = useThreadScroll(
    scroller,
    scrollKey,
  );
  const [zone, setZone] = useState<HTMLDivElement | null>(null);
  const drops = useFileDrops(zone);

  // The pad under the content: the floating footer's height and the gap above it, backed by the
  // background under the footer and fading out above it, so text fades as it scrolls under.
  const pad = (
    <div
      ref={setPad}
      aria-hidden
      className="thread-bottom-pad pointer-events-none sticky bottom-0 z-10 mt-auto w-full shrink-0"
    >
      <ScrollFade edge="bottom" />
    </div>
  );

  return (
    <ThreadScrollContext.Provider value={scroller}>
      <ThreadKeyContext.Provider value={scrollKey}>
      <ThreadPrimitive.Root className="aui-root aui-thread-root bg-background @container flex h-full flex-col">
        <ComposerPrimitive.AttachmentDropzone
          ref={setZone}
          className="group/drop relative flex min-h-0 flex-1 flex-col"
          {...drops.handlers}
        >
          {/* What shows stays put while content changes around it, and the view follows new
              content only from the bottom (see `thread-scroll.ts`). Focusable, so the keys
              scroll it once clicked. */}
          <div
            ref={setViewport}
            data-slot="aui_thread-viewport"
            tabIndex={-1}
            className="thread-scroll-padding flex min-h-0 flex-1 flex-col overflow-x-hidden overflow-y-auto focus:outline-none"
          >
            {/* The column: the pane may move it aside for the pinned summary. */}
            <div
              ref={setColumn}
              data-slot="aui_thread-column"
              className={cn(
                "max-w-thread mx-auto flex min-h-full w-full shrink-0 flex-col px-4",
                Find && !isEmpty ? "pt-thread-find-top" : "pt-8",
              )}
            >
              {/* The new chat: the hero ends a little above the middle, the composer sits at
                  the bottom; each takes half the height. */}
              <AuiIf condition={isNewChatView}>
                <div className="flex min-h-fit grow basis-0 flex-col items-center justify-end pb-11">
                  <Welcome />
                </div>
              </AuiIf>
              <AuiIf condition={isHistoryLoadingView}>
                <ThreadHistoryLoading />
              </AuiIf>
              {BeforeMessages && <BeforeMessages />}

              <ThreadMessages
                setGroup={setGroup}
                group={group}
                viewport={viewport}
                scroller={scroller}
                scrollKey={scrollKey}
              />
              {/* Room for the newest turn's answer, made when the user sends. */}
              <div ref={setSpacer} aria-hidden data-slot="aui_thread-spacer" className="shrink-0" />

              {isEmpty ? <div className="flex grow basis-0 flex-col justify-end">{pad}</div> : pad}
            </div>
          </div>
          {/* Under the top bar the thread fades out (see `scroll-fade.tsx`). */}
          <ScrollFade edge="top" />
          {/* The user's messages, beside the column (see `message-rail.tsx`). */}
          <MessageRail viewport={viewport} scroller={scroller} />
          {Find && <Find viewport={viewport} scroller={scroller} />}

          {/* The composer, with what sits above it, floats over the thread's bottom; its height
              and a gap are the thread's bottom padding. */}
          <div
            ref={setFooter}
            data-thread-scroll-footer
            className="aui-thread-viewport-footer pointer-events-none absolute inset-x-0 bottom-0 z-10 pb-4"
          >
            <div
              data-slot="aui_thread-footer-column"
              className="group/footer max-w-thread relative mx-auto flex w-full flex-col px-4"
            >
              <ThreadScrollToBottom />
              {Capsule && <Capsule />}
              <div className="pointer-events-auto flex flex-col gap-4">
                {/* A new chat has no response to go to (#564). */}
                {!isEmpty && <LatestResponse />}
                {AboveComposer && <AboveComposer />}
                <Composer autoFocus={autoFocus} placeholder={placeholder} />
              </div>
            </div>
          </div>
          {!drops.shift && <DropOverlay native={drops.native} />}
        </ComposerPrimitive.AttachmentDropzone>
      </ThreadPrimitive.Root>
      </ThreadKeyContext.Provider>
    </ThreadScrollContext.Provider>
  );
};

/** A message's height before it was ever measured: a user's message and a finished turn's. */
const ESTIMATE_PX = 160;
/** Messages mounted beyond the view's ends, each way. */
const OVERSCAN = 3;
/** Messages assistant-ui holds beyond those mounted, each way, when the list asks it again... */
const HOLD_MORE = 40;
/** ...which it does once what is mounted comes this near the end of what it holds. */
const HOLD_EDGE = 12;
/** Each conversation's messages' heights as last measured, by id, kept for the app's run. */
const heightsByKey = new Map<string, Map<string, number>>();

function heightsOf(key: string | null): Map<string, number> {
  if (key === null) return new Map();
  let heights = heightsByKey.get(key);
  if (!heights) {
    heights = new Map();
    heightsByKey.set(key, heights);
  }
  return heights;
}

/* One components object for every render: a message's row keeps its message drawn only while it
   stays the same, so a new one each time would draw every message again on each streamed word. */
const MESSAGE_COMPONENTS = { Message: () => <ThreadMessage /> };

const NO_IDS: readonly string[] = [];

/** What a thread asks to mount as it is shown: the message its kept place starts at. */
function restoring(key: string | null): ReadonlyMap<string, string> {
  const kept = keptPlace(key);
  return new Map(kept && !kept.bottom && kept.anchor !== null ? [["restore", kept.anchor]] : []);
}

/** The ids of the messages assistant-ui holds, made once per messages array. */
const heldBy = new WeakMap<readonly { id: string }[], Set<string>>();
function heldIds(messages: readonly { id: string }[]): Set<string> {
  let ids = heldBy.get(messages);
  if (!ids) {
    ids = new Set(messages.map((message) => message.id));
    heldBy.set(messages, ids);
  }
  return ids;
}

/** A row's place on the branch, for what its message reads of the one before it. */
const RowPlaceContext = createContext<number | null>(null);

/**
 * The messages, as a feed: only those near the view are mounted (with `OVERSCAN` more each way),
 * the room of those that aren't kept by spacers, so the thread scrolls as if all were there. Also
 * mounted wherever the view is: the newest turn (its user's message to the end, which the scroller
 * places, follows and measures), the message the view's kept place starts at, and what code going
 * to a message asks for (`ThreadScroller.mount`). The scroller alone moves the view: a message
 * mounting above it at its measured height is a change above the anchor, kept in place there.
 * While the thread is hidden nothing is measured, and what is mounted stays. The ids are the whole
 * branch's (`thread-list.ts`): assistant-ui holds only what the list asks for, `HOLD_MORE` more
 * each way than it mounts, asked again as what it mounts nears the end of what is held.
 */
const ThreadMessages: FC<{
  setGroup: (element: HTMLDivElement | null) => void;
  group: HTMLDivElement | null;
  viewport: HTMLDivElement | null;
  scroller: ThreadScroller;
  scrollKey: string | null;
}> = ({ setGroup, group, viewport, scroller, scrollKey }) => {
  const source = useThreadListSource();
  // Another conversation's, just after a switch (the app tells the list after drawing): none yet.
  const current = useThreadList((s) => s.key === scrollKey);
  const branch = useThreadListIds();
  const ids = !source || current ? branch : NO_IDS;
  // The message the conversation asks to show (a jump between sessions'), mounted where it is.
  const showing = useContext(ThreadHistoryContext)?.showing ?? null;
  const running = useAuiState((s) => s.thread.isRunning);
  // What assistant-ui holds, drawn again as that changes (not on every streamed word).
  const aui = useAui();
  useAuiState((s) => `${s.thread.messages.length}:${s.thread.messages[0]?.id ?? ""}:${s.thread.messages.at(-1)?.id ?? ""}`);
  const newestTurn = useThreadList((s) => s.messages.findLastIndex((message) => message.role === "user"));
  const heights = useMemo(() => heightsOf(scrollKey), [scrollKey]);
  const at = useMemo(() => new Map(ids.map((id, index) => [id, index])), [ids]);
  // What code asked to be mounted, by purpose; the kept place's message to begin with, anew in
  // each conversation shown.
  const [asked, setAsked] = useState<ReadonlyMap<string, string>>(() => restoring(scrollKey));
  const [askedIn, setAskedIn] = useState(scrollKey);
  if (askedIn !== scrollKey) {
    setAskedIn(scrollKey);
    setAsked(restoring(scrollKey));
  }
  // Until the scroller has put the view in its place (the bottom, or the kept place), only what
  // is pinned mounts: not the rows of wherever the view was before the conversation's rows came.
  const placing = useRef(false);
  const shownKey = useRef<string | null | undefined>(undefined);
  if (ids.length > 0 && shownKey.current !== scrollKey) {
    shownKey.current = scrollKey;
    placing.current = true;
  }
  // Placed, it draws again with the rows of where the view is: the virtualizer itself draws again
  // only when that range changes, and a view the scroller left where it was (a thread shorter than
  // the view) never moves. That one is placed by the next frame.
  const [, placed] = useState(0);
  const place = useCallback(() => {
    if (!placing.current) return;
    placing.current = false;
    placed((count) => count + 1);
  }, []);
  const anyIds = ids.length > 0;
  useEffect(() => {
    if (!anyIds) return;
    const next = requestAnimationFrame(place);
    return () => cancelAnimationFrame(next);
  }, [scrollKey, anyIds, place]);
  useLayoutEffect(() => {
    scroller.setMounter((purpose, id) =>
      setAsked((before) => {
        if ((before.get(purpose) ?? null) === id) return before;
        const next = new Map(before);
        if (id === null) next.delete(purpose);
        else next.set(purpose, id);
        return next;
      }),
    );
    return () => scroller.setMounter(null);
  }, [scroller]);
  // Where the list starts in the scrolled content: under the column's padding.
  const [margin, setMargin] = useState(0);
  // Read after every draw, as what is above it may change any time; set only when it moved.
  // oxlint-disable-next-line react-hooks/exhaustive-deps
  useLayoutEffect(() => {
    if (!group || !viewport || viewport.clientHeight === 0) return;
    const top = Math.round(group.getBoundingClientRect().top - viewport.getBoundingClientRect().top + viewport.scrollTop);
    if (top !== margin) setMargin(top);
  });
  const hidden = () => (viewport?.clientHeight ?? 0) === 0;
  const estimate = useCallback((index: number) => heights.get(ids[index] ?? "") ?? ESTIMATE_PX, [heights, ids]);
  const keyOf = useCallback((index: number) => ids[index] ?? index, [ids]);
  const pinned = useMemo(() => {
    const indices = new Set<number>();
    if (newestTurn >= 0) for (let index = newestTurn; index < ids.length; index++) indices.add(index);
    for (const id of [...asked.values(), showing]) {
      const index = id === null ? undefined : at.get(id);
      if (index !== undefined) indices.add(index);
    }
    return indices;
  }, [newestTurn, ids.length, asked, showing, at]);
  const last = useRef<number[]>([]);
  // What the view mounts for where it is, without what is pinned.
  const near = useRef<{ start: number; end: number } | null>(null);
  // The app does not use React Compiler, so the virtualizer's unmemoizable API is fine here.
  // oxlint-disable-next-line react/incompatible-library
  const virtualizer = useVirtualizer({
    count: ids.length,
    getScrollElement: () => viewport,
    estimateSize: estimate,
    getItemKey: keyOf,
    overscan: OVERSCAN,
    scrollMargin: margin,
    // It opens where the scroller will put the view: the kept place's message, or the bottom.
    initialOffset: () => {
      const restore = asked.get("restore");
      const until = restore === undefined ? ids.length : (at.get(restore) ?? ids.length);
      let offset = margin;
      for (let index = 0; index < until; index++) offset += estimate(index);
      return restore === undefined ? Math.max(0, offset - (viewport?.clientHeight ?? 0)) : offset;
    },
    rangeExtractor: (range: VirtualRange) => {
      // Hidden, what is mounted stays: the view's size and place read nothing then.
      if (hidden() && last.current.length > 0) return last.current;
      if (placing.current && pinned.size > 0) {
        last.current = [...pinned].filter((index) => index < range.count).toSorted((a, b) => a - b);
        return last.current;
      }
      const indices = new Set(defaultRangeExtractor(range));
      near.current = indices.size > 0 ? { start: Math.min(...indices), end: Math.max(...indices) } : null;
      for (const index of pinned) if (index < range.count) indices.add(index);
      last.current = [...indices].toSorted((a, b) => a - b);
      return last.current;
    },
    // Hidden, a row measures nothing: it keeps its height.
    measureElement: (element, entry, instance) => {
      const index = instance.indexFromElement(element);
      const id = ids[index];
      if (hidden()) return (id !== undefined ? heights.get(id) : undefined) ?? estimate(index);
      const size = measuredSize(element, entry, instance);
      if (id !== undefined && size > 0) heights.set(id, size);
      return size;
    },
    observeElementRect: (instance, changed) =>
      observeElementRect(instance, (rect) => {
        if (rect.height > 0) changed(rect);
      }),
    // The scroller's own moves are heard as they are made, not a frame later with their scroll
    // event: the rows of where it put the view are drawn before that frame paints.
    observeElementOffset: (instance, changed) => {
      const stop = observeElementOffset(instance, (offset, scrolling) => {
        if (hidden()) return;
        place();
        changed(offset, scrolling);
      });
      const unsubscribe = scroller.subscribe(() => {
        if (!viewport || hidden()) return;
        place();
        changed(viewport.scrollTop, false);
      });
      return () => {
        stop?.();
        unsubscribe();
      };
    },
  });
  // The scroller keeps the view in place as heights change: the list never moves it.
  virtualizer.shouldAdjustScrollPositionOnItemSizeChange = () => false;

  const items = virtualizer.getVirtualItems();
  const holds = source ? heldIds(aui.thread().getState().messages) : null;
  const rows: ReactNode[] = [];
  let end = 0;
  for (const item of items) {
    const id = ids[item.index];
    // A message assistant-ui doesn't hold yet would draw nothing: its room stays a spacer until it does.
    if (id === undefined || (holds !== null && !holds.has(id))) continue;
    const start = item.start - margin;
    if (start > end) rows.push(<div key={`gap:${id}`} aria-hidden data-slot="aui_thread-gap" style={{ height: start - end }} />);
    rows.push(
      <div
        key={id}
        ref={virtualizer.measureElement}
        data-index={item.index}
        data-thread-row={id}
        data-settled={newestTurn >= 0 && item.index < newestTurn ? "" : undefined}
        role="article"
        aria-posinset={item.index + 1}
        aria-setsize={ids.length}
        tabIndex={-1}
        className={cn("focus:outline-none", item.index > 0 && "pt-6")}
      >
        <RowPlaceContext.Provider value={item.index}>
          <RowScope rowKey={rowKeyOf(scrollKey, id)}>
            <ThreadPrimitive.Unstable_MessageById messageId={id} components={MESSAGE_COMPONENTS} />
          </RowScope>
        </RowPlaceContext.Provider>
      </div>,
    );
    end = item.end - margin;
  }
  const total = virtualizer.getTotalSize();
  if (rows.length > 0 && total > end) rows.push(<div key="gap:end" aria-hidden data-slot="aui_thread-gap" style={{ height: total - end }} />);

  // assistant-ui holds what is mounted, with room each way: asked again only as what is mounted
  // nears the end of what it holds, or mounts what it was never asked for.
  useLayoutEffect(() => {
    if (!source || ids.length === 0) return;
    // The newest turn the app holds itself.
    const low = near.current?.start ?? ids.length - 1;
    const high = near.current?.end ?? ids.length - 1;
    const wanted = [...asked.values(), showing].filter((id): id is string => id !== null);
    const held = source.held;
    if (
      held &&
      held.key === scrollKey &&
      (held.from <= 0 || low - HOLD_EDGE >= held.from) &&
      (held.to >= ids.length - 1 || high + HOLD_EDGE <= held.to) &&
      wanted.every((id) => held.ids.includes(id) || ((at.get(id) ?? -1) >= held.from && (at.get(id) ?? -1) <= held.to))
    ) {
      return;
    }
    source.hold({ key: scrollKey, from: Math.max(0, low - HOLD_MORE), to: Math.min(ids.length - 1, high + HOLD_MORE), ids: wanted });
  });

  const feedKeys = useFeedKeys(ids, scroller);
  // The message holding the focus stays mounted while the view scrolls away from it.
  const focused = (event: ReactFocusEvent<HTMLDivElement>) => {
    const row = event.target instanceof Element ? event.target.closest<HTMLElement>("[data-thread-row]") : null;
    const id = ids[Number(row?.dataset["index"])];
    if (id !== undefined) scroller.mount("focus", id);
  };
  return (
    // A feed takes its keys itself: Page Down and Page Up move between its articles (WAI-ARIA's feed).
    // oxlint-disable-next-line jsx-a11y/no-noninteractive-element-interactions
    <div
      ref={setGroup}
      data-slot="aui_message-group"
      role="feed"
      aria-busy={running || undefined}
      aria-label="Conversation"
      onKeyDown={feedKeys}
      onFocus={focused}
      className="flex flex-col empty:hidden"
    >
      {rows}
    </div>
  );
};

/**
 * The feed's keys: Page Down and Page Up in a message move the focus to the next or the one
 * before, mounted first when it isn't (the view goes with the focus).
 */
function useFeedKeys(ids: readonly string[], scroller: ThreadScroller) {
  return useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      if (event.key !== "PageDown" && event.key !== "PageUp") return;
      if (event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) return;
      const target = event.target instanceof Element ? event.target : null;
      if (!target || target.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"])')) return;
      const row = target.closest<HTMLElement>("[data-thread-row]");
      const index = Number(row?.dataset["index"]);
      if (!row || Number.isNaN(index)) return;
      const next = index + (event.key === "PageDown" ? 1 : -1);
      const id = ids[next];
      if (id === undefined) return;
      event.preventDefault();
      const group = event.currentTarget;
      scroller.release();
      scroller.mount("focus", id);
      const focus = (tries: number) => {
        const element = group.querySelector<HTMLElement>(`:scope > [data-thread-row][data-index="${next}"]`);
        if (element) {
          element.focus({ preventScroll: true });
          element.scrollIntoView({ block: "nearest" });
          return;
        }
        if (tries > 0) requestAnimationFrame(() => focus(tries - 1));
      };
      requestAnimationFrame(() => focus(30));
    },
    [ids, scroller],
  );
}

const ThreadMessage: FC = () => {
  const { AssistantMessage } = useThreadComponents();
  const role = useAuiState((s) => s.message.role);
  const editing = useAuiState((s) => s.message.composer.isEditing);

  // A message that fails to render takes nothing else down.
  if (role === "user") return <TurnBoundary>{editing ? <EditComposer /> : <UserMessage />}</TurnBoundary>;
  if (role === "system") return <SystemMessage />;
  return (
    <TurnBoundary>
      <AssistantMessage />
    </TurnBoundary>
  );
};

/** A line from Brigadier itself (an environment problem, a fallback), not from a model. */
const SystemMessage: FC = () => (
  <MessagePrimitive.Root
    data-slot="aui_system-message-root"
    data-role="system"
    className="text-muted-foreground flex justify-center px-2 text-center text-xs"
  >
    <p className="max-w-4/5 whitespace-pre-wrap">
      <MessagePrimitive.Parts />
    </p>
  </MessagePrimitive.Root>
);

/** Three dots in a wave, while the model works below. */
const WorkingDots: FC = () => (
  <span aria-hidden className="flex items-center justify-center gap-1">
    <span className="wave-dot bg-foreground/70 size-1 rounded-full" />
    <span className="wave-dot bg-foreground/70 size-1 rounded-full" />
    <span className="wave-dot bg-foreground/70 size-1 rounded-full" />
  </span>
);

/**
 * ↓ over the composer while a message reaches below the view; "•••" in its place while the model
 * works, and ↓ again on hover or focus. It fades in and out.
 */
const ThreadScrollToBottom: FC = () => {
  const scroller = useContext(ThreadScrollContext);
  const running = useAuiState((s) => s.thread.isRunning);
  const shown = useSyncExternalStore(
    scroller?.subscribe ?? noSubscribe,
    scroller?.contentBelow ?? notShown,
  );
  return (
    <button
      type="button"
      aria-label="Scroll to bottom"
      aria-hidden={!shown || undefined}
      tabIndex={shown ? undefined : -1}
      onClick={shown ? scroller?.scrollToBottom : undefined}
      className={cn(
        "aui-thread-scroll-to-bottom group/scroll border-border bg-background text-foreground above-composer absolute end-1/2 z-30 flex size-8 translate-x-1/2 items-center justify-center rounded-full border bg-clip-padding transition-opacity duration-150 ease-in-out [&_svg]:size-4",
        shown ? "pointer-events-auto opacity-100" : "pointer-events-none opacity-0",
      )}
    >
      {running ? (
        <>
          <span className="group-hover/scroll:hidden group-focus-visible/scroll:hidden">
            <WorkingDots />
          </span>
          <ArrowDown className="hidden group-hover/scroll:block group-focus-visible/scroll:block" />
        </>
      ) : (
        <ArrowDown />
      )}
    </button>
  );
};

const noSubscribe = () => () => {};
const notShown = () => false;

// The markdown stack (remark, micromark, mdast) loads off the cold start (`markdownText`).
const MarkdownText = markdownText.View;

/** Keep Markdown source out of the first reply while its renderer loads. */
const ReplyLoading: FC = () => (
  <div data-slot="reply-loading" role="status" aria-label="Loading reply" className="flex flex-col gap-2 py-1">
    <Skeleton className="h-4 w-4/5 motion-reduce:animate-none" />
    <Skeleton className="h-4 w-3/5 motion-reduce:animate-none" />
  </div>
);

/** A message's file links open what existed when it was written (`LinkTimeContext`). */
const MessageTime: FC<{ children: ReactNode }> = ({ children }) => {
  const sentAt = useAuiState((s) => s.message.createdAt.getTime());
  return <LinkTimeContext.Provider value={sentAt}>{children}</LinkTimeContext.Provider>;
};

export const MessageText: FC<TextMessagePartProps> = (props) => (
  <MessageTime>
    <Suspense fallback={<ReplyLoading />}>
      <MarkdownText {...props} plainLeads />
    </Suspense>
  </MessageTime>
);

/**
 * Markdown outside a thread message, such as a worker's replies in its panel. `plainLeads`: an
 * answer, whose line-opening bold shows plain; `paced`: off, streaming text shows as it arrives
 * (see `MarkdownText`).
 */
export const MarkdownBlock: FC<{
  text: string;
  streaming?: boolean;
  paced?: boolean;
  breaks?: boolean;
  plainLeads?: boolean;
}> = ({ text, streaming = false, paced = true, breaks = false, plainLeads = false }) => (
  <TextMessagePartProvider text={text} isRunning={streaming}>
    <Suspense fallback={<p className="whitespace-pre-wrap">{text}</p>}>
      <MarkdownText streaming={streaming} paced={paced} breaks={breaks} plainLeads={plainLeads} />
    </Suspense>
  </TextMessagePartProvider>
);

/** Text that is still streaming: its newest words fade in. */
export const StreamingMessageText: FC<TextMessagePartProps> = (props) => (
  <MessageTime>
    <Suspense fallback={<ReplyLoading />}>
      <MarkdownText {...props} streaming plainLeads />
    </Suspense>
  </MessageTime>
);

/** A line between a turn's steps while it streams: its words fade in as they arrive, unpaced. */
export const CommentaryMessageText: FC<TextMessagePartProps> = (props) => (
  <MessageTime>
    <Suspense fallback={<ReplyLoading />}>
      <MarkdownText {...props} streaming paced={false} plainLeads />
    </Suspense>
  </MessageTime>
);

const UserAttachments: FC = () => {
  const { UserAttachments: Attachments } = useThreadComponents();
  return Attachments ? <Attachments /> : null;
};

const UserMessage: FC = () => {
  const id = useAuiState((s) => s.message.id);
  const { PeerMessage } = useThreadComponents();
  const peer = useAuiState((s) => s.message.metadata.custom["peer"] !== undefined);
  if (peer && PeerMessage) {
    return (
      <MessagePrimitive.Root
        data-slot="aui_user-message-root"
        data-message-id={id}
        className="scroll-mt-thread-turn flex flex-col items-end gap-y-1 px-2"
        data-role="user"
      >
        <DaySeparator />
        <PeerMessage />
      </MessagePrimitive.Root>
    );
  }
  return (
    <MessagePrimitive.Root
      data-slot="aui_user-message-root"
      data-message-id={id}
      className="group/user scroll-mt-thread-turn flex flex-col items-end gap-y-1 px-2"
      data-role="user"
    >
      <DaySeparator />
      <div className="aui-user-message-content-wrapper flex max-w-7/10 min-w-0 flex-col items-end gap-y-1">
        <UserAttachments />
        <UserMessageText />
        {/* Flush under the bubble: with the turn's gap, its work header starts 48px below the bubble.
            Shown only on hover, so Find in the chat leaves its time out. */}
        <div data-thread-find-skip className="aui-user-action-bar-wrapper peer-empty:hidden -mt-1 opacity-0 transition-opacity group-hover/user:opacity-100 group-focus-within/user:opacity-100">
          <UserActionBar />
        </div>
      </div>
    </MessagePrimitive.Root>
  );
};

/**
 * For a screen reader, before the composer: "Latest response" moves the focus to the newest
 * answer, or says there is none yet. Seen by no one else.
 */
const LatestResponse: FC = () => {
  const [none, setNone] = useState(false);
  const focus = (event: ReactMouseEvent<HTMLButtonElement>) => {
    // The newest turn is always mounted (see `ThreadMessages`); the view alone, as Find's index
    // draws messages in the thread's root too.
    const thread = event.currentTarget.closest(".aui-thread-root")?.querySelector("[data-slot=aui_thread-viewport]") ?? document;
    const answer = [...thread.querySelectorAll<HTMLElement>("[data-thread-row] [data-role=assistant]")].at(-1);
    setNone(!answer);
    answer?.focus();
  };
  return (
    <h2 className="sr-only">
      <button type="button" data-slot="latest-response" onClick={focus}>
        Latest response
      </button>
      <span role="status">{none ? "No response available" : ""}</span>
    </h2>
  );
};

/**
 * "Yesterday 3:09 AM", centred above a message sent more than an hour after the one before it,
 * or above the first when that was more than an hour ago.
 */
const DaySeparator: FC = () => {
  const sentAt = useAuiState((s) => s.message.createdAt.getTime());
  // The message before it on the branch, which assistant-ui may not hold.
  const place = useContext(RowPlaceContext);
  const held = useAuiState((s) => s.message.index);
  const index = place ?? held;
  const previous = useThreadList((s) => (index > 0 ? (s.messages[index - 1]?.createdAt.getTime() ?? null) : null));
  const now = useNow(60_000);
  if (!separatesTurn(sentAt, previous, now)) return null;
  return <TimeSeparator atMs={sentAt} className="pt-2 pb-4" />;
};

/**
 * "Today 3:36 PM", centred: where more than an hour passed, above a user's message or inside
 * the thread's turn (#130).
 */
export const TimeSeparator: FC<{ atMs: number; className?: string }> = ({ atMs, className }) => {
  const now = useNow(60_000);
  return (
    <p data-slot="time-separator" className={cn("text-muted-foreground self-stretch text-center text-sm", className)}>
      {formatDaySeparator(atMs, now)}
    </p>
  );
};

/** The user's text in its bubble; a long message is clipped until "Show more". */
const UserMessageText: FC = () => {
  const { UserText } = useThreadComponents();
  const ref = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useKeptState("more", false);
  const [clipped, setClipped] = useState(false);
  const hasText = useAuiState((s) =>
    s.message.parts.some((part) => part.type === "text" && part.text !== ""),
  );
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    // The cut is 20 lines down, or the end of a code block crossing it, so no block is cut through.
    const measure = () => {
      element.style.maxHeight = "";
      if (expanded) return setClipped(false);
      const cut = element.clientHeight;
      const top = element.getBoundingClientRect().top;
      let end = cut;
      for (const block of element.querySelectorAll("pre")) {
        const box = block.getBoundingClientRect();
        if (box.top - top < cut && box.bottom - top > cut) end = Math.max(end, Math.ceil(box.bottom - top));
      }
      if (end > cut) element.style.maxHeight = `${end}px`;
      setClipped(element.scrollHeight > end + 1);
    };
    measure();
    // Its content grows under a fixed cut (a code block laid out late): watched as well.
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    if (element.firstElementChild) observer.observe(element.firstElementChild);
    return () => observer.disconnect();
  }, [expanded]);
  // Files only: an empty bubble hides itself (and the action bar beside it).
  if (!hasText) return <div className="aui-user-message-content peer empty:hidden" />;
  return (
    // Capped at the column, and any unbroken run (a terminal's `++++…`, a long path) may
    // break anywhere: a bubble sized to such a run overflowed the column leftward, clipped.
    <div className="aui-user-message-content peer bg-muted text-foreground rounded-bubble flex max-w-full min-w-0 flex-col px-4 py-2.5 empty:hidden">
      <div
        ref={ref}
        className={cn(
          // Its own renderer (Markdown, in the conversation) lays the lines out; plain text keeps them.
          "wrap-anywhere [&_.aui-md-inline-code]:bg-foreground/10",
          !UserText && "whitespace-pre-wrap",
          // Cut below only: a list's numbers hang left of its text ("10.") and show whole.
          !expanded && "max-h-user-message overflow-y-clip",
        )}
      >
        <div>{UserText ? <MessagePrimitive.Parts components={{ Text: UserText }} /> : <MessagePrimitive.Parts />}</div>
      </div>
      {(clipped || expanded) && (
        <button
          type="button"
          aria-expanded={expanded}
          onClick={() => setExpanded(!expanded)}
          className="text-muted-foreground hover:text-foreground flex items-center gap-0.5 self-start pt-1 text-xs"
        >
          {expanded ? "Show less" : "Show more"}
          <ChevronRightAlt aria-hidden className={cn("size-icon-xs transition-transform", expanded ? "-rotate-90" : "rotate-90")} />
        </button>
      )}
    </div>
  );
};

/** Whether the message may be edited or answered again now (the view decides, per message). */
const canRework = (s: AssistantState) => s.message.metadata.custom["rework"] === true;

const UserActionBar: FC = () => {
  const text = useAuiState((s) => s.message.parts.map((part) => (part.type === "text" ? part.text : "")).join(""));
  const sentAt = useAuiState((s) => s.message.createdAt.getTime());
  const now = useNow(60_000);
  return (
    <ActionBarPrimitive.Root className="aui-user-action-bar-root text-muted-foreground flex items-center gap-1">
      <span className="pe-1 text-xs tabular-nums">{formatSentAt(sentAt, now)}</span>
      <CopyButton text={text} tooltip="Copy message" copiedDuration={1500} className="aui-user-action-copy" />
      <AuiIf condition={canRework}>
        <ActionBarPrimitive.Edit asChild>
          <TooltipIconButton tooltip="Edit message" className="aui-user-action-edit">
            <EditPencil />
          </TooltipIconButton>
        </ActionBarPrimitive.Edit>
      </AuiIf>
      <BranchPicker />
    </ActionBarPrimitive.Root>
  );
};

/** "‹ 2/3 ›" between the versions of a message (edits, or answers given again). */
export const BranchPicker: FC = () => {
  return (
    <BranchPickerPrimitive.Root
      hideWhenSingleBranch
      className="aui-branch-picker-root text-muted-foreground inline-flex items-center text-xs"
    >
      <BranchPickerPrimitive.Previous asChild>
        <TooltipIconButton tooltip="Previous version">
          <ChevronLeft />
        </TooltipIconButton>
      </BranchPickerPrimitive.Previous>
      <span className="tabular-nums">
        <BranchPickerPrimitive.Number />/<BranchPickerPrimitive.Count />
      </span>
      <BranchPickerPrimitive.Next asChild>
        <TooltipIconButton tooltip="Next version">
          <ChevronRight />
        </TooltipIconButton>
      </BranchPickerPrimitive.Next>
    </BranchPickerPrimitive.Root>
  );
};

/** A sent message being edited: the text in place, then cancel or send the new version. */
const EditComposer: FC = () => {
  return (
    <MessagePrimitive.Root
      data-slot="aui_edit-composer-root"
      data-role="user"
      className="scroll-mt-thread-turn flex flex-col px-2"
    >
      {/* The bubble becomes a composer the column's width: its fill and corners, Cancel an
          outlined pill and Send a filled one. */}
      <ComposerPrimitive.Root data-slot="edit-composer" className="bg-composer rounded-composer ms-auto flex w-full flex-col gap-2 p-2">
        <ComposerPrimitive.Input
          autoFocus
          aria-label="Edit message"
          className="text-foreground max-h-user-message min-h-composer w-full resize-none bg-transparent px-2 py-1 text-base outline-none"
        />
        <div className="flex items-center justify-end gap-2">
          <ComposerPrimitive.Cancel asChild>
            <Button variant="ghost" size="sm" className="border-border rounded-capsule border font-normal">
              Cancel
            </Button>
          </ComposerPrimitive.Cancel>
          <ComposerPrimitive.Send asChild>
            <Button size="sm" className="rounded-capsule font-normal">Send</Button>
          </ComposerPrimitive.Send>
        </div>
      </ComposerPrimitive.Root>
    </MessagePrimitive.Root>
  );
};
