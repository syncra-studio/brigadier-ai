import {
  ActionBarPrimitive,
  AuiIf,
  type AssistantState,
  BranchPickerPrimitive,
  ComposerPrimitive,
  ErrorPrimitive,
  MessagePrimitive,
  TextMessagePartProvider,
  type TextMessagePartProps,
  ThreadPrimitive,
  useAuiEvent,
  useAuiState,
} from "@assistant-ui/react";
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronLeft,
  ChevronRight,
  Copy,
  EditPencil,
  Paperclip,
  Stop,
} from "@openai/apps-sdk-ui/components/Icon";
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
} from "react";

import { ThreadScroller } from "@/components/assistant-ui/thread-scroll";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useNow } from "@/hooks/use-now";
import { formatDaySeparator, formatSentAt, sameDay } from "@/lib/format";
import { cn } from "@/lib/utils";

/**
 * Optional overrides: `AssistantMessage`, `Welcome` and `Composer` replace whole sections;
 * `BeforeMessages` renders above the message list (e.g. a "load earlier" control);
 * `MessageFooter` renders under each assistant message; `UserAttachments` above a user
 * message's text (its files);
 * `AboveComposer` renders between the messages and the composer (queue, notices).
 */
export type ThreadComponents = {
  AssistantMessage?: ComponentType | undefined;
  Welcome?: ComponentType | undefined;
  BeforeMessages?: ComponentType | undefined;
  MessageFooter?: ComponentType | undefined;
  UserAttachments?: ComponentType | undefined;
  AboveComposer?: ComponentType | undefined;
  /** A user message's text, where the app shows what it mentions. */
  UserText?: ComponentType<TextMessagePartProps> | undefined;
  /** Floats centred just above the composer, over the thread (the capsule). */
  Capsule?: ComponentType | undefined;
  Composer?: ComponentType<ComposerProps> | undefined;
};

export type ComposerProps = { autoFocus: boolean; placeholder: string };

export type ThreadProps = {
  components?: ThreadComponents | undefined;
  autoFocus?: boolean | undefined;
  placeholder?: string | undefined;
  /** Where the thread was scrolled is kept under this key (the conversation) while it is away. */
  scrollKey?: string | undefined;
};

const EMPTY_COMPONENTS: ThreadComponents = {};

const ThreadComponentsContext =
  createContext<ThreadComponents>(EMPTY_COMPONENTS);

// Startup exposes a loading placeholder thread; treat it as a new chat so
// the composer mounts centered. Loads after startup keep the docked layout.
const isNewChatView = (s: AssistantState) =>
  s.thread.messages.length === 0 &&
  (!s.thread.isLoading || s.threads.isLoading);

// A switched thread that is still fetching its history: skeleton, not welcome.
const isHistoryLoadingView = (s: AssistantState) =>
  s.thread.messages.length === 0 &&
  s.thread.isLoading &&
  !s.thread.isDisabled &&
  !s.threads.isLoading;

const ThreadHistorySkeleton: FC = () => (
  <div
    data-slot="aui_thread-history-skeleton"
    role="status"
    className="animate-in fade-in fill-mode-both flex flex-col gap-y-6 delay-150 duration-200"
  >
    <span className="sr-only">Loading conversation</span>
    <Skeleton className="h-control-lg ms-auto w-2/5 rounded-xl motion-reduce:animate-none" />
    <div className="flex flex-col gap-y-2">
      <Skeleton className="h-4 w-11/12 motion-reduce:animate-none" />
      <Skeleton className="h-4 w-4/5 motion-reduce:animate-none" />
      <Skeleton className="h-4 w-3/5 motion-reduce:animate-none" />
    </div>
    <Skeleton className="h-control-lg ms-auto w-1/3 rounded-xl motion-reduce:animate-none" />
    <div className="flex flex-col gap-y-2">
      <Skeleton className="h-4 w-10/12 motion-reduce:animate-none" />
      <Skeleton className="h-4 w-2/3 motion-reduce:animate-none" />
    </div>
  </div>
);

export const Thread: FC<ThreadProps> = ({
  components = EMPTY_COMPONENTS,
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
        turnTop: (row) =>
          row instanceof HTMLElement ? Number.parseFloat(getComputedStyle(row).scrollMarginTop) || 0 : 0,
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
    for (const type of ["wheel", "touchstart", "pointerdown", "keydown"]) {
      viewport.addEventListener(type, scroller.interrupt, options);
    }
    // Capturing: the toggle's place is taken before its fold changes.
    viewport.addEventListener(
      "click",
      (event) => {
        if (event.target instanceof HTMLElement) scroller.clicked(event.target);
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

  return { setViewport, setColumn, setGroup, setSpacer, setPad, setFooter };
}

/** "Drop to attach", over the thread while files are dragged onto it. */
const DropOverlay: FC = () => (
  <div
    aria-hidden
    data-slot="drop-overlay"
    className="bg-background/80 pointer-events-none absolute inset-0 z-30 hidden p-3 backdrop-blur-sm group-data-[dragging=true]/drop:flex"
  >
    <div className="border-border rounded-dialog animate-in fade-in-0 flex flex-1 flex-col items-center justify-center gap-2 border-2 border-dashed duration-150">
      <Paperclip className="text-muted-foreground size-icon-lg" />
      <p className="text-lg">Drop to attach</p>
    </div>
  </div>
);

const ThreadRoot: FC<{
  isEmpty: boolean;
  autoFocus: boolean;
  placeholder: string;
  scrollKey: string | null;
}> = ({ isEmpty, autoFocus, placeholder, scrollKey }) => {
  const {
    Welcome = ThreadWelcome,
    BeforeMessages,
    AboveComposer,
    Capsule,
    Composer: ComposerComponent = Composer,
  } = useContext(ThreadComponentsContext);
  const [scroller] = useState(() => new ThreadScroller());
  const { setViewport, setColumn, setGroup, setSpacer, setPad, setFooter } = useThreadScroll(
    scroller,
    scrollKey,
  );

  // The pad under the content: the floating footer's height and the gap above it, backed by the
  // background under the footer and fading out above it, so text fades as it scrolls under.
  const pad = (
    <div
      ref={setPad}
      aria-hidden
      className="thread-bottom-pad pointer-events-none sticky bottom-0 z-10 mt-auto w-full shrink-0"
    >
      <div className="thread-bottom-fade absolute inset-x-0 bottom-0" />
    </div>
  );

  return (
    <ThreadScrollContext.Provider value={scroller}>
      <ThreadPrimitive.Root className="aui-root aui-thread-root bg-background @container flex h-full flex-col">
        <ComposerPrimitive.AttachmentDropzone className="group/drop relative flex min-h-0 flex-1 flex-col">
          {/* What shows stays put while content changes around it, and the view follows new
              content only from the bottom (see `thread-scroll.ts`). Focusable, so the keys
              scroll it once clicked. */}
          <div
            ref={setViewport}
            data-slot="aui_thread-viewport"
            tabIndex={-1}
            className="thread-scroll-padding flex min-h-0 flex-1 flex-col overflow-x-hidden overflow-y-auto focus:outline-none"
          >
            {/* Under the top bar the thread fades out. */}
            <div aria-hidden className="pointer-events-none sticky top-0 z-10 h-0 shrink-0">
              <div className="thread-top-fade absolute inset-x-0 top-0" />
            </div>
            {/* The column: the pane may move it aside for the pinned summary. */}
            <div
              ref={setColumn}
              data-slot="aui_thread-column"
              className="max-w-thread mx-auto flex min-h-full w-full shrink-0 flex-col px-4 pt-8"
            >
              {/* The new chat: the hero ends a little above the middle, the composer sits at
                  the bottom; each takes half the height. */}
              <AuiIf condition={isNewChatView}>
                <div className="flex min-h-fit grow basis-0 flex-col items-center justify-end pb-11">
                  <Welcome />
                </div>
              </AuiIf>
              <AuiIf condition={isHistoryLoadingView}>
                <ThreadHistorySkeleton />
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
                {AboveComposer && <AboveComposer />}
                <ComposerComponent autoFocus={autoFocus} placeholder={placeholder} />
              </div>
            </div>
          </div>
          <DropOverlay />
        </ComposerPrimitive.AttachmentDropzone>
      </ThreadPrimitive.Root>
    </ThreadScrollContext.Provider>
  );
};

const ThreadMessage: FC = () => {
  const { AssistantMessage: AssistantMessageComponent = AssistantMessage } =
    useContext(ThreadComponentsContext);
  const role = useAuiState((s) => s.message.role);
  const editing = useAuiState((s) => s.message.composer.isEditing);

  if (role === "user") return editing ? <EditComposer /> : <UserMessage />;
  if (role === "system") return <SystemMessage />;
  return <AssistantMessageComponent />;
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
 * ↓ over the composer once the view is above the bottom; "•••" in its place while the model
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

const ThreadWelcome: FC = () => {
  return (
    <div className="aui-thread-welcome-root mb-6 flex flex-col px-2">
      <p className="aui-thread-welcome-message-inner fade-in slide-in-from-bottom-1 animate-in fill-mode-both font-display text-2xl font-medium tracking-tight duration-200">
        How can I help you today?
      </p>
    </div>
  );
};

const Composer: FC<ComposerProps> = ({
  autoFocus,
  placeholder,
}) => {
  return (
    <ComposerPrimitive.Root className="aui-composer-root relative flex w-full flex-col">
      <div
        data-slot="aui_composer-shell"
        className="border-foreground/10 focus-within:border-foreground/25 bg-muted/30 rounded-thread flex w-full cursor-text flex-col gap-2 border p-2 transition-[border-color]"
      >
        <ComposerPrimitive.Input
          placeholder={placeholder}
          className="aui-composer-input caret-primary placeholder:text-muted-foreground/60 min-h-composer max-h-48 w-full resize-none bg-transparent px-2.5 py-1 text-base outline-none"
          rows={1}
          autoFocus={autoFocus}
          enterKeyHint="send"
          aria-label="Message input"
        />
        <ComposerAction />
      </div>
    </ComposerPrimitive.Root>
  );
};

const ComposerAction: FC = () => {
  return (
    <div className="aui-composer-action-wrapper relative flex items-center justify-end gap-1.5">
      <AuiIf condition={(s) => !s.composer.canCancel}>
        <ComposerPrimitive.Send asChild>
          <TooltipIconButton
            tooltip="Send message"
            side="bottom"
            type="button"
            variant="default"
            size="icon-md"
            className="aui-composer-send rounded-capsule"
            aria-label="Send message"
          >
            <ArrowUp className="aui-composer-send-icon" />
          </TooltipIconButton>
        </ComposerPrimitive.Send>
      </AuiIf>
      <AuiIf condition={(s) => s.composer.canCancel}>
        <ComposerPrimitive.Cancel asChild>
          <Button
            type="button"
            variant="default"
            size="icon-md"
            className="aui-composer-cancel rounded-capsule"
            aria-label="Stop generating"
          >
            <Stop className="aui-composer-cancel-icon size-icon-sm" />
          </Button>
        </ComposerPrimitive.Cancel>
      </AuiIf>
    </div>
  );
};

export const MessageError: FC = () => {
  return (
    <MessagePrimitive.Error>
      <ErrorPrimitive.Root className="aui-message-error-root border-destructive bg-destructive/5 text-destructive mt-2 rounded-md border p-3 text-sm">
        <ErrorPrimitive.Message className="aui-message-error-message line-clamp-2" />
      </ErrorPrimitive.Root>
    </MessagePrimitive.Error>
  );
};

// The markdown stack (remark, micromark, mdast) loads with the first reply, not at startup.
const MarkdownText = lazy(() =>
  import("@/components/assistant-ui/markdown-text").then((module) => ({
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

export const MessageText: FC<TextMessagePartProps> = (props) => (
  <Suspense fallback={<ReplyLoading />}>
    <MarkdownText {...props} />
  </Suspense>
);

/** Markdown outside a thread message, such as a worker's replies in its panel. */
export const MarkdownBlock: FC<{ text: string; streaming?: boolean }> = ({
  text,
  streaming = false,
}) => (
  <TextMessagePartProvider text={text} isRunning={streaming}>
    <Suspense fallback={<p className="whitespace-pre-wrap">{text}</p>}>
      <MarkdownText streaming={streaming} />
    </Suspense>
  </TextMessagePartProvider>
);

/** Text that is still streaming: its newest words fade in. */
export const StreamingMessageText: FC<TextMessagePartProps> = (props) => (
  <Suspense fallback={<ReplyLoading />}>
    <MarkdownText {...props} streaming />
  </Suspense>
);

const AssistantMessage: FC = () => {
  return (
    <MessagePrimitive.Root
      data-slot="aui_assistant-message-root"
      data-role="assistant"
      className="relative -mb-7.5 pb-7.5 last:mb-0"
    >
      <div
        data-slot="aui_assistant-message-content"
        className="text-foreground px-2 leading-relaxed wrap-break-word"
      >
        <MessagePrimitive.Parts components={{ Text: MessageText }} />
        <MessageError />
      </div>

      <div
        data-slot="aui_assistant-message-footer"
        className="ms-2 flex min-h-7.5 items-center gap-2 pt-1.5"
      >
        <AssistantActionBar />
        <MessageFooter />
      </div>
    </MessagePrimitive.Root>
  );
};

const UserAttachments: FC = () => {
  const { UserAttachments: Attachments } = useContext(ThreadComponentsContext);
  return Attachments ? <Attachments /> : null;
};

const MessageFooter: FC = () => {
  const { MessageFooter: Footer } = useContext(ThreadComponentsContext);
  return Footer ? <Footer /> : null;
};

const CopyIcon: FC = () => (
  <>
    <AuiIf condition={(s) => s.message.isCopied}>
      <Check className="animate-in zoom-in-50 fade-in duration-200 ease-out" />
    </AuiIf>
    <AuiIf condition={(s) => !s.message.isCopied}>
      <Copy className="animate-in zoom-in-75 fade-in duration-150" />
    </AuiIf>
  </>
);

const AssistantActionBar: FC = () => {
  return (
    <ActionBarPrimitive.Root
      hideWhenRunning
      autohide="not-last"
      className="aui-assistant-action-bar-root text-muted-foreground animate-in fade-in -ms-1 flex gap-1 duration-200"
    >
      <ActionBarPrimitive.Copy asChild>
        <TooltipIconButton tooltip="Copy">
          <CopyIcon />
        </TooltipIconButton>
      </ActionBarPrimitive.Copy>
    </ActionBarPrimitive.Root>
  );
};

const UserMessage: FC = () => {
  const id = useAuiState((s) => s.message.id);
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
        {/* Flush under the bubble: with the turn's gap, its work header starts 48px below the bubble. */}
        <div className="aui-user-action-bar-wrapper peer-empty:hidden -mt-1 opacity-0 transition-opacity group-hover/user:opacity-100 group-focus-within/user:opacity-100">
          <UserActionBar />
        </div>
      </div>
    </MessagePrimitive.Root>
  );
};

/** "Yesterday 3:09 AM", centred above the first message of a day. */
const DaySeparator: FC = () => {
  const sentAt = useAuiState((s) => s.message.createdAt.getTime());
  const previous = useAuiState((s) => {
    const index = s.message.index;
    return index > 0 ? (s.thread.messages[index - 1]?.createdAt.getTime() ?? null) : null;
  });
  const now = useNow(60_000);
  if (previous === null ? sameDay(sentAt, now) : sameDay(previous, sentAt)) return null;
  return (
    <p className="text-muted-foreground self-stretch pt-2 pb-4 text-center text-sm">
      {formatDaySeparator(sentAt, now)}
    </p>
  );
};

/** The user's text in its bubble; a long message is clipped until "Show more". */
const UserMessageText: FC = () => {
  const { UserText } = useContext(ThreadComponentsContext);
  const ref = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [clipped, setClipped] = useState(false);
  const hasText = useAuiState((s) =>
    s.message.parts.some((part) => part.type === "text" && part.text !== ""),
  );
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => setClipped(element.scrollHeight > element.clientHeight + 1);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  // Files only: an empty bubble hides itself (and the action bar beside it).
  if (!hasText) return <div className="aui-user-message-content peer empty:hidden" />;
  return (
    // Capped at the column, and any unbroken run (a terminal's `++++…`, a long path) may
    // break anywhere: a bubble sized to such a run overflowed the column leftward, clipped.
    <div className="aui-user-message-content peer bg-muted text-foreground rounded-bubble flex max-w-full min-w-0 flex-col px-4 py-2.5 empty:hidden">
      <div
        ref={ref}
        className={cn(
          "whitespace-pre-wrap wrap-anywhere",
          !expanded && "max-h-user-message overflow-hidden",
        )}
      >
        {UserText ? <MessagePrimitive.Parts components={{ Text: UserText }} /> : <MessagePrimitive.Parts />}
      </div>
      {(clipped || expanded) && (
        <button
          type="button"
          aria-expanded={expanded}
          onClick={() => setExpanded(!expanded)}
          className="text-muted-foreground hover:text-foreground self-start pt-1 text-xs"
        >
          {expanded ? "Show less" : "Show more"}
        </button>
      )}
    </div>
  );
};

/** Whether the message may be edited or answered again now (the view decides, per message). */
const canRework = (s: AssistantState) => s.message.metadata.custom["rework"] === true;

const UserActionBar: FC = () => {
  const sentAt = useAuiState((s) => s.message.createdAt.getTime());
  const now = useNow(60_000);
  return (
    <ActionBarPrimitive.Root className="aui-user-action-bar-root text-muted-foreground flex items-center gap-1">
      <span className="pe-1 text-xs tabular-nums">{formatSentAt(sentAt, now)}</span>
      <ActionBarPrimitive.Copy asChild>
        <TooltipIconButton tooltip="Copy" className="aui-user-action-copy">
          <CopyIcon />
        </TooltipIconButton>
      </ActionBarPrimitive.Copy>
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
      <ComposerPrimitive.Root className="bg-muted rounded-thread ms-auto flex w-full flex-col gap-2 p-2">
        <ComposerPrimitive.Input
          autoFocus
          aria-label="Edit message"
          className="text-foreground max-h-user-message min-h-composer w-full resize-none bg-transparent px-2 py-1 text-base outline-none"
        />
        <div className="flex items-center justify-end gap-2">
          <ComposerPrimitive.Cancel asChild>
            <Button variant="ghost" size="sm">
              Cancel
            </Button>
          </ComposerPrimitive.Cancel>
          <ComposerPrimitive.Send asChild>
            <Button size="sm">Send</Button>
          </ComposerPrimitive.Send>
        </div>
      </ComposerPrimitive.Root>
    </MessagePrimitive.Root>
  );
};
