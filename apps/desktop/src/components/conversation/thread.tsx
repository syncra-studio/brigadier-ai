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
  useAuiEvent,
  useAuiState,
} from "@assistant-ui/react";
import {
  createContext,
  lazy,
  Suspense,
  useContext,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ComponentType,
  type FC,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";

import { useFileDrops } from "@/app/conversation/fileDrops";
import { CopyButton } from "@/components/shared/copy-button";
import { LinkTimeContext } from "@/components/conversation/file-links";
import { MessageRail } from "@/components/conversation/message-rail";
import { ThreadScroller } from "@/components/conversation/thread-scroll";
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

export const Thread: FC<ThreadProps> = ({
  components,
  autoFocus = true,
  placeholder = "Send a message...",
  scrollKey,
}) => {
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
};

const ThreadScrollContext = createContext<ThreadScroller | null>(null);

/**
 * Wires the scroll element, its column, the message rows, the room after them, the pad under
 * them and the floating footer to the thread's scroll controller (see `thread-scroll.ts`).
 */
function useThreadScroll(
  scroller: ThreadScroller,
  saveKey: string | null,
): {
  viewport: HTMLDivElement | null;
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
    const list = () => [...group.children].filter((child) => child instanceof HTMLElement);
    let rows = list();
    // The pad and the scroll padding follow the floating footer's height (see globals.css).
    const footerResized = () =>
      viewport.style.setProperty("--thread-footer-height", `${footer.offsetHeight}px`);
    footerResized();
    const detach = scroller.attach(
      {
        viewport,
        rows: () => rows,
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
    const children = new MutationObserver(() => {
      rows = list();
      scroller.rowsChanged();
    });
    children.observe(group, { childList: true });

    const listening = new AbortController();
    const options = { passive: true, signal: listening.signal };
    viewport.addEventListener("scroll", () => scroller.scrolled(), options);
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

  return { viewport, setViewport, setColumn, setGroup, setSpacer, setPad, setFooter };
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
  const { viewport, setViewport, setColumn, setGroup, setSpacer, setPad, setFooter } = useThreadScroll(
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

              <div
                ref={setGroup}
                data-slot="aui_message-group"
                className="flex flex-col gap-y-6 empty:hidden"
              >
                <ThreadPrimitive.Messages>
                  {() => <ThreadMessage />}
                </ThreadPrimitive.Messages>
              </div>
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
                <LatestResponse />
                {AboveComposer && <AboveComposer />}
                <Composer autoFocus={autoFocus} placeholder={placeholder} />
              </div>
            </div>
          </div>
          {!drops.shift && <DropOverlay native={drops.native} />}
        </ComposerPrimitive.AttachmentDropzone>
      </ThreadPrimitive.Root>
    </ThreadScrollContext.Provider>
  );
};

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

// The markdown stack (remark, micromark, mdast) loads with the first reply, not at startup.
const MarkdownText = lazy(() =>
  import("@/components/conversation/markdown-text").then((module) => ({
    default: module.MarkdownText,
  })),
);

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
 * answer, whose line-opening bold shows plain (see `MarkdownText`).
 */
export const MarkdownBlock: FC<{ text: string; streaming?: boolean; breaks?: boolean; plainLeads?: boolean }> = ({
  text,
  streaming = false,
  breaks = false,
  plainLeads = false,
}) => (
  <TextMessagePartProvider text={text} isRunning={streaming}>
    <Suspense fallback={<p className="whitespace-pre-wrap">{text}</p>}>
      <MarkdownText streaming={streaming} breaks={breaks} plainLeads={plainLeads} />
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
    const thread = event.currentTarget.closest(".aui-thread-root") ?? document;
    const answer = [...thread.querySelectorAll<HTMLElement>("[data-role=assistant]")].at(-1);
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
  const previous = useAuiState((s) => {
    const index = s.message.index;
    return index > 0 ? (s.thread.messages[index - 1]?.createdAt.getTime() ?? null) : null;
  });
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
  const [expanded, setExpanded] = useState(false);
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
