import { useAui } from "@assistant-ui/react";
import {
  Branch,
  CheckCircle,
  Copy,
  DotsHorizontal,
  FolderOpen,
  HandRaised,
  Link,
  Plus,
  PullRequestClosed,
  PullRequestDraft,
  PullRequestMerged,
  PullRequestOpen,
  Tasks,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  type ReactNode,
  type UIEvent,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useShallow } from "zustand/react/shallow";

import { showCard } from "@/app/conversation/ActionCards";
import { isRunRequest } from "@/app/conversation/blocks";
import { PlanSection } from "@/app/conversation/cards/PlanSection";
import { OvernightPlanCard } from "@/app/conversation/cards/OvernightPlanCard";
import {
  overnightActions,
  shownRun,
  useOvernightCards,
  useRunDiff,
} from "@/app/conversation/overnightAdapter";
import { decisionWords, workerName } from "@/app/conversation/rowWords";
import { keptScroll, useSummary } from "@/app/conversation/summaryState";
import { useAction } from "@/app/conversation/useAction";
import { WorkerLine } from "@/app/conversation/WorkerChip";
import { GitActions } from "@/app/conversation/GitActions";
import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { WorkersSummary } from "@/app/conversation/WorkerSummary";
import { disclosureRow } from "@/components/assistant-ui/elements/surfaces";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Popover,
  PopoverAnchor,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { openFolder, openUrl, request } from "@/ipc/client";
import { Button } from "@/components/ui/button";
import type {
  Conversation,
  Decision,
  DiffStat,
  PullRequest,
  PullRequestState,
  Task,
  WaitingItem,
} from "@/ipc/generated";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import {
  getSessionDiff,
  resolveWaiting,
  select,
  setPinnedSummary,
  setProjectExpanded,
} from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

const NO_TASKS: Readonly<Record<string, Task>> = {};
const NO_WAITING: readonly WaitingItem[] = [];
const NO_DECISIONS: readonly Decision[] = [];

/** How long a reopened summary card keeps restoring its offset while its rows arrive. */
const RESTORE_MS = 1000;

/** The run's card inside a summary card, which is its surface. */
const BARE = "rounded-none border-0 bg-transparent";

/** The branch's +N −N against its base, read again whenever a worker lands. */
function useSessionDiff(
  conversationId: string,
  worktree: boolean,
): DiffStat | null {
  const landed = useBoard(
    (s) =>
      Object.values(s.board?.tasks ?? {}).filter(
        (task) => task.state === "landed",
      ).length,
  );
  const [stat, setStat] = useState<DiffStat | null>(null);
  useEffect(() => {
    if (!worktree) return;
    let current = true;
    getSessionDiff(conversationId)
      .then((next) => current && setStat(next))
      // The card shows the branch alone when git can't tell.
      .catch(() => current && setStat(null));
    return () => {
      current = false;
    };
    // Each landing changes the branch: read it again.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [conversationId, worktree, landed]);
  return worktree ? stat : null;
}

const Section = ({
  title,
  action,
  children,
}: {
  title: string;
  action?: ReactNode;
  children: ReactNode;
}) => (
  <section className="flex flex-col gap-1">
    <h3 className="text-muted-foreground flex h-control-xs items-center justify-between text-xs">
      {title}
      {action}
    </h3>
    {children}
  </section>
);

/** Web links in `text`, without trailing punctuation. */
function linksIn(text: string): string[] {
  return (text.match(/https?:\/\/[^\s<>()"'`]+/g) ?? []).map((url) =>
    url.replace(/[.,;:!?]+$/, ""),
  );
}

function hostOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/**
 * The session's sources, listed in the pinned card: links in the user's messages (as soon as
 * they are sent) and pages the orchestrator read, first seen first.
 */
function useSources(conversationId: string): string[] {
  const messages = useApp((s) => s.threads[conversationId]?.items);
  const read = useBoard(
    useShallow((s) =>
      s.board?.conversationId === conversationId
        ? s.board.orchestratorSteps.flatMap((step) =>
            step.kind.type === "readPage" ? [step.kind.url] : [],
          )
        : [],
    ),
  );
  return useMemo(() => {
    const said = (messages ?? []).flatMap((message) =>
      message.role === "user" ? linksIn(message.text) : [],
    );
    return [...new Set([...said, ...read])];
  }, [messages, read]);
}

function openLink(url: string): void {
  openUrl(url).catch((error: unknown) =>
    toast(error instanceof Error ? error.message : String(error), {
      tone: "error",
    }),
  );
}

const SourceRow = ({ url, dim }: { url: string; dim?: boolean }) => (
  <button
    type="button"
    title={url}
    onClick={() => openLink(url)}
    className={cn(
      "hover:bg-foreground/5 rounded-control -mx-1 flex h-control-sm items-center gap-2 px-1 text-start text-sm transition-colors",
      dim && "text-muted-foreground",
    )}
  >
    <Link aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
    <span className="min-w-0 truncate">{hostOf(url)}</span>
  </button>
);

/** Sources shown before "View all". */
const SOURCES = 3;

/** "Sources": the first links, "View all" for every one, and "+" to add one to the message. */
function Sources({ conversationId }: { conversationId: string }) {
  const sources = useSources(conversationId);
  const aui = useAui();
  if (sources.length === 0) return null;
  const add = () => {
    const composer = aui.composer();
    const text = composer.getState().text;
    composer.setText(
      text && !text.endsWith(" ") ? `${text} https://` : `${text}https://`,
    );
    requestAnimationFrame(() =>
      document.querySelector<HTMLElement>(COMPOSER_EDITABLE)?.focus(),
    );
  };
  return (
    <Section
      title="Sources"
      action={
        <TooltipIconButton tooltip="Add source" size="icon-xs" onClick={add}>
          <Plus />
        </TooltipIconButton>
      }
    >
      {sources.slice(0, SOURCES).map((url) => (
        <SourceRow key={url} url={url} />
      ))}
      <Popover>
        <PopoverTrigger asChild>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground -mx-1 flex h-control-sm items-center gap-2 px-1 text-start text-sm transition-colors"
          >
            <Link aria-hidden className="size-icon-sm shrink-0 opacity-60" />
            View all
          </button>
        </PopoverTrigger>
        <PopoverContent
          align="end"
          className="flex max-h-80 w-xs flex-col overflow-y-auto"
        >
          {sources.map((url) => (
            <button
              key={url}
              type="button"
              onClick={() => openLink(url)}
              className="hover:bg-muted rounded-control flex flex-col px-2 py-1 text-start"
            >
              <span className="truncate text-sm">{hostOf(url)}</span>
              <span className="text-muted-foreground truncate text-xs">
                {url}
              </span>
            </button>
          ))}
        </PopoverContent>
      </Popover>
    </Section>
  );
}

/** Where an item waiting on the user came from, in a few words. */
function waitingFrom(
  item: WaitingItem,
  tasks: Readonly<Record<string, Task>>,
): string | null {
  switch (item.source.type) {
    case "task":
    case "landing": {
      const task = tasks[item.source.taskId];
      if (!task) return null;
      return item.source.type === "task"
        ? `From ${workerName(tasks, task)}`
        : `Before ${workerName(tasks, task)} can land`;
    }
    case "card":
      return "A card waits for your answer";
    case "orchestrator":
      return null;
    case "run":
      return "Declined during the overnight run";
  }
}

/**
 * One thing only the user can do, on one plain line that opens to its whole text and where it
 * came from; Done (or Show, for a card).
 */
function WaitingRow({
  item,
  conversationId,
}: {
  item: WaitingItem;
  conversationId: string;
}) {
  const tasks = useBoard((s) => s.board?.tasks ?? NO_TASKS);
  const action = useAction();
  const from = waitingFrom(item, tasks);
  const [open, setOpen] = useState(false);
  const { source } = item;
  return (
    <div className="flex flex-col gap-0.5 py-0.5">
      <div className="flex items-start gap-2 text-sm">
        <HandRaised
          aria-hidden
          className="text-muted-foreground mt-0.5 size-icon-sm shrink-0"
        />
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className={cn(
            "rounded-control focus-visible:ring-ring/50 min-w-0 flex-1 text-start outline-none focus-visible:ring-1",
            open ? "wrap-break-word" : "truncate",
          )}
        >
          <WorkerLine text={item.what} />
        </button>
        {source.type === "card" && (
          <Button
            size="xs"
            variant="ghost"
            onClick={() => showCard(conversationId, source.cardId)}
          >
            Show
          </Button>
        )}
        <Button
          size="xs"
          variant="ghost"
          disabled={action.busy}
          onClick={() =>
            action.run(() => resolveWaiting(conversationId, item.id))
          }
        >
          Done
        </Button>
      </div>
      {open && from && (
        <span className="text-muted-foreground ps-6 text-xs">{from}</span>
      )}
      {action.error && (
        <span className="text-destructive ps-6 text-xs">{action.error}</span>
      )}
    </div>
  );
}

/** "Waiting on you": what only the user can do, oldest first, each until they mark it done. */
function WaitingOnYou({ conversationId }: { conversationId: string }) {
  const items = useBoard(
    useShallow((s) =>
      s.board?.conversationId === conversationId
        ? Object.values(s.board.waiting).toSorted(
            (a, b) => a.createdAtMs - b.createdAtMs,
          )
        : NO_WAITING,
    ),
  );
  if (items.length === 0) return null;
  return (
    <Section title="Waiting on you">
      {items.map((item) => (
        <WaitingRow key={item.id} item={item} conversationId={conversationId} />
      ))}
    </Section>
  );
}

/** Decisions shown before "Show all". */
const DECISIONS = 5;

/** A decision on one plain line, which opens to its whole text and why. */
function DecisionRow({ decision }: { decision: Decision }) {
  const [open, setOpen] = useState(false);
  const { what, why } = decisionWords(decision);
  return (
    <button
      type="button"
      aria-expanded={open}
      onClick={() => setOpen(!open)}
      className={cn(disclosureRow, "flex items-start gap-2 py-0.5 text-sm")}
    >
      <CheckCircle
        aria-hidden
        className="text-muted-foreground mt-0.5 size-icon-sm shrink-0"
      />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className={open ? "wrap-break-word" : "truncate"}><WorkerLine text={what} /></span>
        {open && why && (
          <span className="text-muted-foreground text-xs wrap-break-word">
            <WorkerLine text={why} />
          </span>
        )}
      </span>
    </button>
  );
}

/** "Decided for you": what Brigadier decided on the user's behalf, newest first. */
function DecidedForYou({ conversationId }: { conversationId: string }) {
  const decisions = useBoard((s) =>
    s.board?.conversationId === conversationId
      ? s.board.decisions
      : NO_DECISIONS,
  );
  const [all, setAll] = useState(false);
  if (decisions.length === 0) return null;
  const newest = decisions.toReversed();
  return (
    <Section title="Decided for you">
      {(all ? newest : newest.slice(0, DECISIONS)).map((decision) => (
        <DecisionRow key={decision.id} decision={decision} />
      ))}
      {newest.length > DECISIONS && (
        <button
          type="button"
          onClick={() => setAll(!all)}
          className="text-muted-foreground hover:text-foreground -mx-1 flex h-control-sm items-center px-1 text-start text-sm transition-colors"
        >
          {all ? "Show fewer" : `Show all ${newest.length}`}
        </button>
      )}
    </Section>
  );
}

/** The project's ⋯ "Actions": a new session, its folder, and its path. */
function ProjectActions({
  projectId,
  path,
}: {
  projectId: string;
  path: string;
}) {
  const mac = useApp((s) => s.info?.platform === "macos");
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton tooltip="Actions" size="icon-xs">
          <DotsHorizontal />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem
          onSelect={() => {
            setProjectExpanded(projectId, true);
            select({ type: "draft", kind: "session", projectId });
          }}
        >
          <Plus />
          New session
        </DropdownMenuItem>
        <DropdownMenuItem
          onSelect={() =>
            void openFolder(path).catch((error: unknown) =>
              toast(error instanceof Error ? error.message : String(error), {
                tone: "error",
              }),
            )
          }
        >
          <FolderOpen />
          {mac ? "Reveal in Finder" : "Open in File Manager"}
        </DropdownMenuItem>
        <DropdownMenuItem
          onSelect={() =>
            navigator.clipboard.writeText(path).then(
              () => toast("Copied path"),
              () => toast("Failed to copy path", { tone: "error" }),
            )
          }
        >
          <Copy />
          Copy path
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

const PULL_REQUEST: Record<
  PullRequestState,
  { label: string; icon: ReactNode }
> = {
  open: { label: "Open", icon: <PullRequestOpen /> },
  draft: { label: "Draft", icon: <PullRequestDraft /> },
  merged: { label: "Merged", icon: <PullRequestMerged /> },
  closed: { label: "Closed", icon: <PullRequestClosed /> },
};

/** The branch's GitHub pull request (from `gh`, read only), looked up again after landings. */
function usePullRequest(conversationId: string): PullRequest | null {
  const landed = useBoard(
    (s) =>
      Object.values(s.board?.tasks ?? {}).filter(
        (task) => task.state === "landed",
      ).length,
  );
  const [found, setFound] = useState<PullRequest | null>(null);
  useEffect(() => {
    let current = true;
    request({ method: "getPullRequest", conversationId })
      .then(({ pullRequest }) => current && setFound(pullRequest))
      // No row when it can't be told.
      .catch(() => current && setFound(null));
    return () => {
      current = false;
    };
    // A landing may be what a pull request waits for: look again.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [conversationId, landed]);
  return found;
}

/** The pull request row: the branch's pull request, opened in the browser. */
function PullRequestRow({ pullRequest }: { pullRequest: PullRequest }) {
  const state = PULL_REQUEST[pullRequest.state];
  return (
    <button
      type="button"
      title={`${pullRequest.title}\n${pullRequest.url}`}
      onClick={() =>
        openUrl(pullRequest.url).catch((cause: unknown) =>
          toast(cause instanceof Error ? cause.message : String(cause), {
            tone: "error",
          }),
        )
      }
      className="hover:bg-foreground/5 rounded-control -mx-1 flex h-control-sm items-center gap-2 px-1 text-start text-sm transition-colors [&>svg]:size-icon-md [&>svg]:shrink-0"
    >
      {state.icon}
      <span className="min-w-0 flex-1 truncate">
        <span className="text-muted-foreground">#{pullRequest.number}</span>{" "}
        {pullRequest.title}
      </span>
      <span className="text-muted-foreground shrink-0 text-xs">
        {state.label}
      </span>
    </button>
  );
}

/**
 * Where the thread's pane puts the pinned summary, by the room either side of the thread's
 * column: beside the column, beside it with the column moved aside to make room, or (too little
 * room) floating over the thread, opened from the top bar.
 */
type SummaryLayout = "beside" | "shift" | "float";

/**
 * The thread's pane, laid out for the pinned summary when `summary` (a session's own view). It
 * follows its width without rendering: the layout goes on the element for CSS (the column's move,
 * the card's visibility), and to the store only when it changes, for the top bar's button. The
 * first layout lands before the pane eases anything, so a thread opens already in place.
 */
export function SummaryPane({
  summary,
  children,
}: {
  summary: boolean;
  children: ReactNode;
}) {
  const pinned = useApp((s) => s.pinnedSummary);
  const pane = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = pane.current;
    if (!summary || !element) return;
    const column = tokenPx("--container-thread");
    const floatBelow = tokenPx("--spacing-summary-float-below");
    const besideFrom = tokenPx("--spacing-summary-beside-from");
    let frame = 0;
    const observer = new ResizeObserver(([entry]) => {
      if (!entry) return;
      const room = (entry.contentRect.width - column) / 2;
      const layout: SummaryLayout =
        room < floatBelow ? "float" : room < besideFrom ? "shift" : "beside";
      element.dataset.summary = layout;
      if (!("settled" in element.dataset)) {
        frame = requestAnimationFrame(() => {
          element.dataset.settled = "";
        });
      }
      // Only a floating summary stays open over the thread.
      useSummary.setState((state) =>
        state.layout === layout
          ? state
          : { layout, floating: state.floating && layout === "float" },
      );
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
      delete element.dataset.summary;
      delete element.dataset.settled;
      useSummary.setState({ layout: "beside", floating: false });
    };
  }, [summary]);
  return (
    <div
      ref={pane}
      data-pinned={(summary && pinned) || undefined}
      className="group/pane relative min-h-0 flex-1 overflow-x-clip data-pinned:data-[summary=shift]:thread-column:-translate-x-(--spacing-summary-shift) motion-safe:data-settled:thread-column:transition-[translate] motion-safe:data-settled:thread-column:duration-350 motion-safe:data-settled:thread-column:ease-summary-shift"
    >
      {children}
    </div>
  );
}

/**
 * An element of the summary that reopens where it was scrolled to, kept under `key`. While it is
 * `active`, every scroll counts, a clamp too (so a card that stops overflowing forgets its
 * offset). Just after it shows it is restoring instead: rows that read asynchronously (the
 * branch's diff, a pull request) may not be there yet, so the offset is applied again as they
 * arrive, until it is reached, the element is scrolled by hand, or a moment has passed.
 */
function useKeptScroll(key: string, active: boolean) {
  const ref = useRef<HTMLDivElement>(null);
  const restoring = useRef(false);
  const onScroll = (event: UIEvent<HTMLElement>) => {
    const { scrollTop } = event.currentTarget;
    if (!active) return;
    if (restoring.current) {
      if (scrollTop !== keptScroll.get(key)) return;
      restoring.current = false;
    }
    if (scrollTop > 0) keptScroll.set(key, scrollTop);
    else keptScroll.delete(key);
  };
  useLayoutEffect(() => {
    const element = ref.current;
    const top = keptScroll.get(key) ?? 0;
    if (!active || !element || top === 0) return;
    restoring.current = true;
    const restore = () => {
      if (restoring.current && element.scrollTop !== top) element.scrollTop = top;
    };
    const stop = () => {
      restoring.current = false;
    };
    restore();
    const observer = new ResizeObserver(restore);
    observer.observe(element);
    if (element.firstElementChild) observer.observe(element.firstElementChild);
    const timer = setTimeout(stop, RESTORE_MS);
    for (const type of ["wheel", "pointerdown", "keydown"]) element.addEventListener(type, stop);
    return () => {
      stop();
      observer.disconnect();
      clearTimeout(timer);
      for (const type of ["wheel", "pointerdown", "keydown"]) element.removeEventListener(type, stop);
    };
  }, [active, key]);
  return { ref, onScroll };
}

/**
 * The summary's column of cards, pinned in the pane or floating from the top bar: inset from the
 * top bar and the pane's end, each card as tall as what it shows. Where the cards can't all fit at
 * their least height the column scrolls too, so every card stays reachable.
 */
const STACK =
  "pointer-events-none flex max-h-full min-h-0 w-summary-stack max-w-full flex-col gap-summary-inset overflow-y-auto p-summary-inset";

/**
 * One of the summary's cards, floating over the thread: as tall as what it shows, and scrolling
 * on its own once the column has no room left for all of it. It shrinks no shorter than
 * `--spacing-summary-card-min` (or its own height, when that is less).
 */
function SummaryCard({
  scrollKey,
  active,
  children,
}: {
  scrollKey: string;
  active: boolean;
  children: ReactNode;
}) {
  const { ref, onScroll } = useKeptScroll(scrollKey, active);
  const [height, setHeight] = useState<number | null>(null);
  useLayoutEffect(() => {
    const content = ref.current?.firstElementChild;
    if (!content) return;
    const observer = new ResizeObserver(([entry]) => {
      const size = entry?.borderBoxSize[0]?.blockSize;
      if (size !== undefined) setHeight(size);
    });
    observer.observe(content);
    return () => observer.disconnect();
  }, [ref]);
  return (
    <div
      data-slot="summary-card"
      style={
        height === null
          ? undefined
          : { minHeight: `min(${height}px, var(--spacing-summary-card-min))` }
      }
      className="bg-card rounded-summary shadow-summary pointer-events-auto flex min-w-0 flex-col overflow-hidden"
    >
      <div ref={ref} onScroll={onScroll} className="min-h-0 overflow-y-auto">
        <div className="flex min-w-0 flex-col">{children}</div>
      </div>
    </div>
  );
}

/**
 * A session's summary, pinned at the top end of its thread's pane: the project, the branch (with
 * what it changed, for a worktree session), the plan and the workers, and a run's own card. It
 * eases in from the pane's end when pinned and out when unpinned. Where the pane has too little
 * room beside the thread's column it hides, and the same cards float over the thread instead,
 * opened from the top bar.
 */
export function PinnedSummary({
  conversation,
}: {
  conversation: Conversation;
}) {
  const shown = useApp((s) => s.pinnedSummary);
  const float = useSummary((s) => s.layout === "float");
  const visible = shown && !float;
  // The cards stay for their easing in and out; what they show is kept only while they can be seen.
  const [content, setContent] = useState(visible);
  if (visible && !content) setContent(true);
  const { ref: stack, onScroll } = useKeptScroll(`${conversation.id}/column`, visible && content);
  useEffect(() => {
    if (visible || !content) return;
    let current = true;
    // Reading the column's animations applies the hiding first, so its easing out is among them.
    const easing = stack.current?.getAnimations() ?? [];
    void Promise.allSettled(easing.map((animation) => animation.finished)).then(
      () => current && setContent(false),
    );
    return () => {
      current = false;
    };
  }, [visible, content, stack]);
  if (conversation.setup?.type !== "session") return null;

  return (
    <>
      <div className="pointer-events-none absolute inset-y-0 end-0 z-10 flex max-w-full flex-col">
        <aside
          ref={stack}
          aria-label="Session summary"
          aria-hidden={!visible || undefined}
          data-state={shown ? "open" : "closed"}
          onScroll={onScroll}
          className={cn(
            STACK,
            "summary-hidden:invisible summary-hidden:translate-x-full summary-hidden:scale-80 summary-hidden:opacity-0 origin-top-right motion-safe:group-data-settled/pane:transition-[opacity,translate,scale,visibility] motion-safe:group-data-settled/pane:duration-300 motion-safe:group-data-settled/pane:ease-summary-card",
          )}
        >
          {content && <SummaryContent conversation={conversation} active={visible} />}
        </aside>
      </div>
      {float && <FloatingSummary conversation={conversation} />}
    </>
  );
}

/**
 * The summary where it floats: the same column of cards over the thread, at the pane's top end
 * under the top bar, reaching at most the window's bottom inset. The top bar's button, Escape or a
 * click elsewhere closes it.
 */
function FloatingSummary({ conversation }: { conversation: Conversation }) {
  const { ref, onScroll } = useKeptScroll(`${conversation.id}/column`, true);
  return (
    <>
      <PopoverAnchor className="pointer-events-none absolute end-0 top-0 size-0" />
      <PopoverContent
        ref={ref}
        side="bottom"
        align="end"
        sideOffset={0}
        aria-label="Session summary"
        onScroll={onScroll}
        // Focus the summary itself, not its first button (whose tip would open with it).
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          if (event.currentTarget instanceof HTMLElement) event.currentTarget.focus();
        }}
        className={cn(
          STACK,
          "text-foreground max-w-(--radix-popover-content-available-width) max-h-(--radix-popover-content-available-height) rounded-none bg-transparent shadow-none ring-0",
        )}
      >
        <SummaryContent conversation={conversation} active />
      </PopoverContent>
    </>
  );
}

/**
 * The summary's popover where it floats, around the top bar (its button) and the pane (where it
 * opens). Elsewhere it stays closed and holds nothing.
 */
export function SummaryFloat({ children }: { children: ReactNode }) {
  const floating = useSummary((s) => s.floating);
  return (
    <Popover open={floating} onOpenChange={(open) => useSummary.setState({ floating: open })}>
      {children}
    </Popover>
  );
}

/** The branch the session's work is on, with its +N −N once known. */
function BranchRow({ branch, diff }: { branch: string; diff: DiffStat | null }) {
  return (
    <div className="flex min-w-0 flex-1 items-center gap-2 text-sm">
      <Branch
        aria-hidden
        className="text-muted-foreground size-icon-md shrink-0"
      />
      <span className="min-w-0 flex-1 truncate" title={branch}>
        {branch}
      </span>
      {diff && (diff.insertions > 0 || diff.deletions > 0) && (
        <span className="shrink-0 text-xs tabular-nums">
          <span className="text-success">+{diff.insertions}</span>{" "}
          <span className="text-destructive">−{diff.deletions}</span>
        </span>
      )}
    </div>
  );
}

/**
 * The summary's cards, pinned in the pane or floating from the top bar: the context card (with
 * the session's plan as one of its sections), then the run's card. `active` while they can be
 * seen, for their scroll offsets.
 */
function SummaryContent({
  conversation,
  active,
}: {
  conversation: Conversation;
  active: boolean;
}) {
  const overnight = useOvernightCards(conversation.id);
  const project = useApp((s) =>
    conversation.projectId
      ? (s.projects[conversation.projectId]?.name ?? null)
      : null,
  );
  const workers = useBoard((s) =>
    s.board?.conversationId === conversation.id
      ? Object.keys(s.board.tasks).length
      : 0,
  );
  // The session's own plans, oldest first. A run's own plans (Phase 0's, each phase lead's) show
  // inside its card instead.
  const planIds = useBoard(
    useShallow((s) =>
      s.board?.conversationId === conversation.id
        ? Object.values(s.board.plans)
            .filter((plan) => plan.state.type !== "superseded" && !isRunRequest(plan.requestId))
            .toSorted((a, b) => a.position - b.position)
            .map((plan) => plan.id)
        : [],
    ),
  );
  const plans = planIds.filter(
    (id) => !overnight.some((card) => card.run.planId === id || card.run.planning?.planId === id),
  );
  const run = shownRun(overnight);
  const setup =
    conversation.setup?.type === "session" ? conversation.setup : null;
  const worktree = setup?.environment.type === "newWorktree";
  const sessionDiff = useSessionDiff(conversation.id, worktree && !run);
  const runDiff = useRunDiff(conversation.id, run?.id ?? null);
  // During and after a run the card shows the run's branch: that is where the work is.
  const diff = run ? runDiff : sessionDiff;
  const sources = useSources(conversation.id).length > 0;
  const waiting = useBoard((s) =>
    s.board?.conversationId === conversation.id
      ? Object.keys(s.board.waiting).length
      : 0,
  );
  const decided = useBoard((s) =>
    s.board?.conversationId === conversation.id ? s.board.decisions.length : 0,
  );
  const pullRequest = usePullRequest(conversation.id);
  if (!setup) return null;
  const checkout =
    setup.environment.type === "newWorktree"
      ? (setup.environment.path ?? setup.repo)
      : setup.repo;

  return (
    <>
      <SummaryCard scrollKey={`${conversation.id}/context`} active={active}>
        <div className="flex flex-col gap-2 px-3 py-2.5">
          <div className="flex h-control-xs items-center gap-2">
            <h2 className="text-muted-foreground min-w-0 flex-1 truncate text-xs">
              {project ?? setup.repo}
            </h2>
            {conversation.projectId && (
              <ProjectActions
                projectId={conversation.projectId}
                path={checkout}
              />
            )}
          </div>
          {run?.workspace ? (
            // The run's card merges its verified work: no second way to merge here.
            <BranchRow branch={run.workspace.branch} diff={diff} />
          ) : (
            <GitActions conversationId={conversation.id}>
              <BranchRow branch={setup.environment.branch} diff={diff} />
            </GitActions>
          )}
          {pullRequest && <PullRequestRow pullRequest={pullRequest} />}
          {(plans.length > 0 || workers > 0 || sources || waiting > 0 || decided > 0) && (
            <div className="border-border border-t" />
          )}
          {plans.length > 0 && <PlanSection planIds={plans} />}
          <WaitingOnYou conversationId={conversation.id} />
          {workers > 0 && <WorkersSummary conversationId={conversation.id} />}
          <DecidedForYou conversationId={conversation.id} />
          <Sources conversationId={conversation.id} />
        </div>
      </SummaryCard>
      {/* The run's card sits right under the context card, before any other plan. */}
      {overnight.map((model) => (
        <SummaryCard key={model.run.id} scrollKey={`${conversation.id}/overnight-${model.run.id}`} active={active}>
          <OvernightPlanCard model={model} actions={overnightActions} className={BARE} />
        </SummaryCard>
      ))}
    </>
  );
}

/**
 * The top bar's summary button. Where the pane keeps the summary beside the thread it pins and
 * unpins it; where the summary floats, it opens it over the thread, under the top bar.
 */
export function PinnedSummaryToggle() {
  const pinned = useApp((s) => s.pinnedSummary);
  const float = useSummary((s) => s.layout === "float");
  const floating = useSummary((s) => s.floating);
  if (!float) {
    return (
      <TooltipIconButton
        tooltip="Toggle pinned summary"
        size="icon-md"
        aria-pressed={pinned}
        className={cn(pinned && "bg-muted")}
        onClick={() => setPinnedSummary(!pinned)}
      >
        <Tasks />
      </TooltipIconButton>
    );
  }
  return (
    <PopoverTrigger asChild>
      <TooltipIconButton
        tooltip="Toggle summary"
        size="icon-md"
        aria-pressed={floating}
        className={cn(floating && "bg-muted")}
      >
        <Tasks />
      </TooltipIconButton>
    </PopoverTrigger>
  );
}
