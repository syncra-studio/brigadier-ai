import { useAui } from "@assistant-ui/react";
import {
  type ReactNode,
  type UIEvent,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useShallow } from "zustand/react/shallow";

import { showCard } from "@/app/conversation/ActionCards";
import { useAskRequest } from "@/app/conversation/activity/askRequest";
import { revertPullRequest } from "@/app/conversation/activity/revert";
import { connectorName } from "@/app/conversation/activity/words";
import { isRunRequest } from "@/app/conversation/blocks";
import { JobCard, WaitCard } from "@/app/conversation/cards/JobCard";
import { useSessionJobs } from "@/app/conversation/boardJob";
import { OvernightPlanCard } from "@/app/conversation/cards/OvernightPlanCard";
import {
  overnightActions,
  shownRun,
  useOvernightCards,
  useRunDiff,
} from "@/app/conversation/overnightAdapter";
import { ActivitiesSummary } from "@/app/conversation/Activities";
import { workerName } from "@/app/conversation/rowWords";
import { keptScroll, openSummaryByItself, useSummary } from "@/app/conversation/summaryState";
import { RightSidebarContext } from "@/app/conversation/RightSidebar";
import {
  Branch,
  Globe,
  HandRaised,
  Link,
  Paperclip,
  Plugin,
  Plus,
  PullRequestClosed,
  PullRequestDraft,
  PullRequestMerged,
  PullRequestOpen,
  Tasks,
} from "@/components/icons";
import { CHAT_TAB, openPanelTab, useSessionTabsOf } from "@/state/sessionTabs";
import { AgentsPanelContext, WorkerLine } from "@/app/conversation/WorkerChip";
import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { WorkersSummary } from "@/app/conversation/WorkerSummary";
import {
  SummaryRow,
  SummaryRowButton,
  SummarySection,
} from "@/components/conversation/elements/summary-section";
import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { Button } from "@/components/controls/button";
import {
  Popover,
  PopoverAnchor,
  PopoverContent,
  PopoverTrigger,
} from "@/components/overlays/popover";
import { request } from "@/ipc/client";
import { openLink as openWebLink } from "@/state/links";
import type {
  Conversation,
  DiffStat,
  PullRequest,
  PullRequestState,
  Task,
  WaitingItem,
} from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import {
  getSessionDiff,
  setPinnedSummary,
} from "@/state/actions";
import { useBoard } from "@/state/board";
import { useNow } from "@/hooks/use-now";
import { summaryShownOf, useApp } from "@/state/store";
import { toast } from "@/state/toasts";

const NO_TASKS: Readonly<Record<string, Task>> = {};
const NO_WAITING: readonly WaitingItem[] = [];

/** How long a reopened summary card keeps restoring its offset while its rows arrive. */
const RESTORE_MS = 1000;

/** The run's card inside a summary card, which is its surface. */
const BARE = "rounded-none border-0 bg-transparent";

/** The branch's +N −N against its base, read again whenever a worker lands. */
function useSessionDiff(
  conversationId: string,
  worktree: boolean,
): { stat: DiffStat; commits: number } | null {
  // Landings, the thread's own requests ending, its commits meanwhile (a fix it makes before the
  // request ends) and merges move the branch.
  const landed = useBoard(
    (s) =>
      Object.values(s.board?.tasks ?? {}).filter((task) => task.state === "landed").length +
      Object.values(s.board?.requests ?? {}).filter((asked) => asked.state.type === "done").length +
      (s.board?.orchestratorSteps ?? []).filter((step) => step.kind.type === "merged").length,
  );
  const tip = useBoard((s) => s.board?.threadTip ?? null);
  const [stat, setStat] = useState<{ stat: DiffStat; commits: number } | null>(null);
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
  }, [conversationId, worktree, landed, tip]);
  return worktree ? stat : null;
}

/** Web links in `text`, without trailing punctuation. */
function linksIn(text: string): string[] {
  return (text.match(/https?:\/\/[^\s<>()"'`]+/g) ?? []).map((url) =>
    url.replace(/[.,;:!?]+$/, ""),
  );
}

export function hostOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/**
 * The session's sources: links in the user's messages (as soon as they are sent), first seen
 * first; in a session also the pages the thread read.
 */
function useSources(conversationId: string, session = true): string[] {
  const messages = useApp((s) => s.threads[conversationId]?.items);
  const read = useBoard(
    useShallow((s) =>
      session && s.board?.conversationId === conversationId
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
  openWebLink(url, { agent: true }).catch((error: unknown) =>
    toast(error instanceof Error ? error.message : String(error), {
      tone: "error",
    }),
  );
}

const SourceRow = ({ url }: { url: string }) => (
  <SummaryRowButton title={url} icon={<Link />} onClick={() => openLink(url)}>
    {hostOf(url)}
  </SummaryRowButton>
);

/** One of what a session used, as its card lists it. */
export type UsedSource = { key: string; icon: ReactNode; label: string; title: string; open?: () => void };

/**
 * What a session used, as its card's Sources list it: the files attached to the user's messages,
 * the links in them, the integrations the thread called, then one "Web search" once it searched
 * or read a page. The pages themselves are in the Sources tab.
 */
export function useUsedSources(conversationId: string): UsedSource[] {
  const messages = useApp((s) => s.threads[conversationId]?.items);
  const json = useBoard((s) => {
    if (s.board?.conversationId !== conversationId) return "[[],false]";
    const steps = s.board.orchestratorSteps;
    const connectors = [...new Set(steps.flatMap((step) => (step.kind.type === "tool" ? [connectorName(step.kind.name)] : []).filter((name) => name !== null)))];
    const web = steps.some((step) => step.kind.type === "searchedWeb" || step.kind.type === "readPage");
    return JSON.stringify([connectors, web]);
  });
  return useMemo(() => {
    const [connectors, web] = JSON.parse(json) as [string[], boolean];
    const users = (messages ?? []).filter((message) => message.role === "user");
    const files = [...new Map(users.flatMap((message) => message.attachments).map((file) => [file.id, file])).values()];
    const links = [...new Set(users.flatMap((message) => linksIn(message.text)))];
    return [
      ...files.map((file): UsedSource => ({ key: `file:${file.id}`, icon: <Paperclip />, label: file.name, title: file.name })),
      ...links.map((url): UsedSource => ({ key: `link:${url}`, icon: <Link />, label: hostOf(url), title: url, open: () => openLink(url) })),
      ...connectors.map((name): UsedSource => ({ key: `tool:${name}`, icon: <Plugin />, label: name, title: name })),
      ...(web ? [{ key: "web", icon: <Globe />, label: "Web search", title: "Web search" }] : []),
    ];
  }, [messages, json]);
}

/** Sources shown before "View all". */
const SOURCES = 3;

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
 * One thing only the user can do, on one line; its whole text and where it came from in its
 * tooltip. A click shows what it waits on: the card, or the worker in the panel. It ends without
 * a Done: when it is over, or with its run.
 */
function WaitingRow({
  item,
  conversationId,
}: {
  item: WaitingItem;
  conversationId: string;
}) {
  const tasks = useBoard((s) => s.board?.tasks ?? NO_TASKS);
  const { setPanel } = useContext(AgentsPanelContext);
  const from = waitingFrom(item, tasks);
  const { source } = item;
  const title = [item.what, from].filter(Boolean).join("\n");
  const show =
    source.type === "card"
      ? () => showCard(conversationId, source.cardId)
      : source.type === "task" || source.type === "landing"
        ? () => setPanel(source.taskId)
        : null;
  return show ? (
    <SummaryRowButton title={title} icon={<HandRaised />} onClick={show}>
      <WorkerLine text={item.what} />
    </SummaryRowButton>
  ) : (
    <SummaryRow title={title} icon={<HandRaised />}>
      <WorkerLine text={item.what} />
    </SummaryRow>
  );
}

/**
 * Whether an item shows: an overnight run's, since only a run keeps a list for the morning. A
 * session lists nothing (its thread says it in its answer), so one listed before that shows
 * nowhere.
 */
function runItem(item: WaitingItem, tasks: Readonly<Record<string, Task>>): boolean {
  const { source } = item;
  if (source.type === "run") return true;
  if (source.type === "task" || source.type === "landing") return !!tasks[source.taskId]?.run;
  return isRunRequest(item.requestId);
}

/**
 * "Waiting on you": an overnight run's list of what only the user can do, oldest first. The
 * section is there only while something waits.
 */
function WaitingOnYou({ conversationId }: { conversationId: string }) {
  const items = useBoard(
    useShallow((s) =>
      s.board?.conversationId === conversationId
        ? Object.values(s.board.waiting)
            .filter((item) => runItem(item, s.board?.tasks ?? NO_TASKS))
            .toSorted((a, b) => a.createdAtMs - b.createdAtMs)
        : NO_WAITING,
    ),
  );
  if (items.length === 0) return null;
  return (
    <SummarySection foldKey="waiting" title="Waiting on you" count={items.length}>
      {items.map((item) => (
        <WaitingRow key={item.id} item={item} conversationId={conversationId} />
      ))}
    </SummarySection>
  );
}

/** "+" on Sources: starts a link in the composer, for the user to finish. */
function useAddSource(): () => void {
  const aui = useAui();
  return () => {
    const composer = aui.composer();
    const text = composer.getState().text;
    composer.setText(
      text && !text.endsWith(" ") ? `${text} https://` : `${text}https://`,
    );
    requestAnimationFrame(() =>
      document.querySelector<HTMLElement>(COMPOSER_EDITABLE)?.focus(),
    );
  };
}

function AddSource() {
  const add = useAddSource();
  return (
    <TooltipIconButton tooltip="Add source" size="icon-xs" onClick={add}>
      <Plus />
    </TooltipIconButton>
  );
}

/**
 * A session's "Sources": what it used (attachments, links, integrations, "Web search"), the first
 * three, then "View all", which opens the Sources tab with every search and page.
 */
function SessionSources({ conversationId }: { conversationId: string }) {
  const used = useUsedSources(conversationId);
  if (used.length === 0) return null;
  return (
    <SummarySection foldKey="sources" title="Sources" count={used.length} action={<AddSource />}>
      {used.slice(0, SOURCES).map((source) =>
        source.open ? (
          <SummaryRowButton key={source.key} title={source.title} icon={source.icon} onClick={source.open}>
            {source.label}
          </SummaryRowButton>
        ) : (
          <SummaryRow key={source.key} title={source.title} icon={source.icon}>
            {source.label}
          </SummaryRow>
        ),
      )}
      {(used.length > SOURCES || used.some((source) => source.key === "web")) && (
        <SummaryRowButton muted icon={<Link />} onClick={() => openPanelTab(conversationId, { type: "sources" }, "Sources")}>
          View all
        </SummaryRowButton>
      )}
    </SummarySection>
  );
}

/** A Chat's "Sources": the links in the user's messages; "View all" lists every one. */
function Sources({ conversationId }: { conversationId: string }) {
  const sources = useSources(conversationId, false);
  if (sources.length === 0) return null;
  return (
    <SummarySection
      foldKey="sources"
      title="Sources"
      count={sources.length}
      action={<AddSource />}
    >
      {sources.slice(0, SOURCES).map((url) => (
        <SourceRow key={url} url={url} />
      ))}
      <Popover>
        <PopoverTrigger asChild>
          <SummaryRowButton muted icon={<Link />}>
            View all
          </SummaryRowButton>
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
    </SummarySection>
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

/** The pull request row: the branch's pull request, opened in the browser, and what to ask of it. */
function PullRequestRow({ conversationId, pullRequest }: { conversationId: string; pullRequest: PullRequest }) {
  const state = PULL_REQUEST[pullRequest.state];
  const asks = pullRequestAsks(pullRequest);
  return (
    <>
      <SummaryRowButton
        title={`${pullRequest.title}\n${pullRequest.url}`}
        icon={state.icon}
        meta={state.label}
        onClick={() =>
          openWebLink(pullRequest.url, { agent: true }).catch((cause: unknown) =>
            toast(cause instanceof Error ? cause.message : String(cause), {
              tone: "error",
            }),
          )
        }
      >
        <span className="text-muted-foreground">#{pullRequest.number}</span>{" "}
        {pullRequest.title}
      </SummaryRowButton>
      {asks.length > 0 && (
        <div data-slot="pr-actions" className="-ms-2 flex flex-wrap gap-0.5 ps-6">
          {asks.map((ask) => (
            <AskButton key={ask.label} conversationId={conversationId} {...ask} />
          ))}
        </div>
      )}
    </>
  );
}

/**
 * What the user can ask of the pull request, by its state: each button sends the session a fixed
 * request as the user's, and the thread carries it out.
 */
function pullRequestAsks(pr: PullRequest): { label: string; text: string }[] {
  const n = `PR #${pr.number}`;
  if (pr.state === "merged") {
    return pr.mergeCommit && pr.base
      ? [{ label: "Revert", text: revertPullRequest(pr.mergeCommit, pr.number, pr.base) }]
      : [];
  }
  if (pr.state === "closed") return [];
  const asks: { label: string; text: string }[] = [];
  if (pr.ci === "failing") asks.push({ label: "Fix CI", text: `Fix the failing CI on ${n}.` });
  if (pr.changesRequested || (pr.unresolved ?? 0) > 0) {
    asks.push({ label: "Address comments", text: `Address the review comments on ${n}.` });
  }
  if (pr.conflicts) asks.push({ label: "Resolve conflicts", text: `Resolve the merge conflicts on ${n}.` });
  else if (pr.state === "open") asks.push({ label: "Merge now", text: `Merge ${n} now.` });
  return asks;
}

/** One of the pull request's buttons: sends its request once, then reads as asked. */
function AskButton({ conversationId, label, text }: { conversationId: string; label: string; text: string }) {
  const { asked, ask } = useAskRequest(conversationId, text);
  return (
    <Button
      size="xs"
      variant="ghost"
      className="text-muted-foreground font-normal"
      title={asked ? "Asked" : text}
      disabled={asked}
      data-asked={asked || undefined}
      onClick={ask}
    >
      {label}
    </Button>
  );
}

/**
 * Where the thread's pane puts the pinned summary, by the room either side of the thread's
 * column: beside the column, beside it with the column moved aside (and narrowed where it must)
 * to leave the card its own gutter, or (a pane too narrow for that) floating over the thread,
 * opened from the top bar.
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
  const pinned = useApp(summaryShownOf);
  const pane = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = pane.current;
    if (!summary || !element) return;
    const column = tokenPx("--container-thread");
    const gutterFrom = tokenPx("--spacing-summary-gutter-from");
    const besideFrom = tokenPx("--spacing-summary-beside-from");
    let frame = 0;
    const observer = new ResizeObserver(([entry]) => {
      if (!entry) return;
      const { width } = entry.contentRect;
      const room = (width - column) / 2;
      const layout: SummaryLayout =
        width < gutterFrom ? "float" : room < besideFrom ? "shift" : "beside";
      element.dataset.summary = layout;
      if (!("settled" in element.dataset)) {
        frame = requestAnimationFrame(() => {
          element.dataset.settled = "";
        });
      }
      // A summary shown stays shown as the window changes: floating where it floats, pinned
      // beside the thread elsewhere.
      const state = useSummary.getState();
      if (state.layout === layout && state.measured) return;
      if (state.layout === "float" && state.floating) setPinnedSummary(true);
      const floating = layout === "float" && (state.floating || summaryShownOf(useApp.getState()));
      useSummary.setState({ layout, measured: true, floating });
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
      delete element.dataset.summary;
      delete element.dataset.settled;
      useSummary.setState({ layout: "beside", measured: false, floating: false });
    };
  }, [summary]);
  return (
    <div
      ref={pane}
      data-pinned={(summary && pinned) || undefined}
      className="group/pane relative min-h-0 flex-1 overflow-x-clip data-pinned:data-[summary=shift]:thread-column:-translate-x-(--spacing-summary-shift) data-pinned:data-[summary=shift]:thread-column:max-w-thread-beside-summary motion-safe:data-settled:thread-column:transition-[translate] motion-safe:data-settled:thread-column:duration-350 motion-safe:data-settled:thread-column:ease-summary-shift"
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
      tabIndex={-1}
      style={
        height === null
          ? undefined
          : { minHeight: `min(${height}px, var(--spacing-summary-card-min))` }
      }
      className="bg-popover rounded-summary shadow-summary pointer-events-auto flex min-w-0 flex-col overflow-hidden"
    >
      <div ref={ref} onScroll={onScroll} className="min-h-0 overflow-y-auto">
        <div className="flex min-w-0 flex-col">{children}</div>
      </div>
    </div>
  );
}

/**
 * A session's summary, pinned at the top end of its thread's pane, in the reference order: the
 * project, what the branch changed, what waits on the user, the workers, what runs in the
 * background and the sources; under it the job at work and what waits on a set time (Q88), then a
 * run's own card. It eases in from the pane's end when pinned and out when unpinned. Where the
 * pane has too little room beside the thread's column it hides, and the same cards float over the
 * thread instead, opened from the top bar. While the context card is hidden, the job's cards
 * stay, in its place.
 */
export function PinnedSummary({
  conversation,
}: {
  conversation: Conversation | null;
}) {
  const shown = useApp(summaryShownOf);
  const float = useSummary((s) => s.layout === "float");
  const visible = shown && !float;
  const floating = useSummary((s) => s.floating);
  // A session's card opens by itself once it has its project and branch to show, never to say
  // there is nothing here yet.
  const measured = useSummary((s) => s.measured);
  const ready = conversation?.kind === "session" && conversation.setup?.type === "session" ? conversation.id : null;
  useEffect(() => {
    if (ready && measured) openSummaryByItself(ready);
  }, [ready, measured]);
  // The cards stay for their easing in and out; what they show is kept only while they can be seen.
  const [content, setContent] = useState(visible);
  if (visible && !content) setContent(true);
  const { ref: stack, onScroll } = useKeptScroll(`${conversation?.id ?? "draft"}/column`, visible && content);
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

  return (
    <>
      {/* Over the thread's top fade (z-10), which comes after it. */}
      <div className="pointer-events-none absolute inset-y-0 end-0 z-20 flex max-w-full flex-col">
        <aside
          ref={stack}
          aria-label="Context"
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
      {conversation?.kind === "session" && !visible && !(float && floating) && (
        <div className="pointer-events-none absolute inset-y-0 end-0 z-20 flex max-w-full flex-col">
          <div aria-label="Job" data-slot="job-stack" className={STACK}>
            <JobStack conversationId={conversation.id} />
          </div>
        </div>
      )}
      {float && <FloatingSummary conversation={conversation} />}
    </>
  );
}

/**
 * The summary where it floats: the same column of cards over the thread, at the pane's top end
 * under the top bar, reaching at most the window's bottom inset. The top bar's button or Escape
 * closes it; a click elsewhere or a resize leaves it shown.
 */
function FloatingSummary({ conversation }: { conversation: Conversation | null }) {
  const { ref, onScroll } = useKeptScroll(`${conversation?.id ?? "draft"}/column`, true);
  // A row that opens the side panel (the workers, a worker) or a tab moves on to it: the summary
  // closes rather than stay where it opened, over the panel, and focus stays off the top bar's
  // button. Coming back to the chat, as the button itself does from another tab, is not moving on.
  const rightSidebar = useContext(RightSidebarContext);
  const { active: mainTab } = useSessionTabsOf(conversation?.id ?? "draft");
  const { panel: worker } = useContext(AgentsPanelContext);
  const panel = `${rightSidebar?.open}/${rightSidebar?.active}/${worker}`;
  const shownPanel = useRef(panel);
  const shownTab = useRef(mainTab);
  const movedOn = useRef(false);
  useEffect(() => {
    const away = panel !== shownPanel.current || (mainTab !== shownTab.current && mainTab !== CHAT_TAB);
    if (away && useSummary.getState().floating) {
      movedOn.current = true;
      useSummary.setState({ floating: false });
    }
    shownPanel.current = panel;
    shownTab.current = mainTab;
  }, [panel, mainTab]);
  return (
    <>
      {/* The pane's whole width, so the card follows the pane's end as the pane narrows (a side
          panel opening) instead of staying where it first opened. */}
      <PopoverAnchor className="pointer-events-none absolute inset-x-0 top-0 h-0" />
      <PopoverContent
        ref={ref}
        side="bottom"
        align="end"
        sideOffset={0}
        aria-label="Context"
        onScroll={onScroll}
        // It stays over the thread while the user reads or types there: only the top bar's
        // button or Escape closes it.
        onInteractOutside={(event) => event.preventDefault()}
        // Focus the summary itself, not its first button (whose tip would open with it).
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          if (event.currentTarget instanceof HTMLElement) event.currentTarget.focus();
        }}
        onCloseAutoFocus={(event) => {
          if (movedOn.current) event.preventDefault();
          movedOn.current = false;
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
    <Popover
      open={floating}
      onOpenChange={(open) => {
        // The user's show or hide holds for every window size.
        setPinnedSummary(open);
        useSummary.setState({ floating: open });
      }}
    >
      {children}
    </Popover>
  );
}

/** The branch the session's work is on, with its +N −N once known. */
function BranchRow({ branch, diff }: { branch: string; diff: DiffStat | null }) {
  return (
    <SummaryRow
      data-row="branch"
      title={branch}
      icon={<Branch />}
      meta={
        diff &&
        (diff.insertions > 0 || diff.deletions > 0) && (
          <span>
            <span className="text-success">+{diff.insertions}</span>{" "}
            <span className="text-destructive">−{diff.deletions}</span>
          </span>
        )
      }
    >
      {branch}
    </SummaryRow>
  );
}

/**
 * The summary's cards, pinned in the pane or floating from the top bar: the context card, the
 * job's cards, then the run's card. `active` while they can be seen, for their scroll offsets.
 */
function SummaryContent({ conversation, active }: { conversation: Conversation | null; active: boolean }) {
  if (!conversation) return <SummaryCard scrollKey="draft/context" active={active}>
    <p className="text-muted-foreground p-4 text-sm">Nothing here yet</p>
  </SummaryCard>;
  return conversation.kind === "session" ? <SessionSummaryContent conversation={conversation} active={active} />
    : <ChatSummaryContent conversation={conversation} active={active} />;
}

/** Plain chats expose their own workers, sources, attachments and usage, without session sections. */
function ChatSummaryContent({ conversation, active }: { conversation: Conversation; active: boolean }) {
  const sources = useSources(conversation.id, false);
  const attachments = useApp(useShallow((s) => [...new Map((s.threads[conversation.id]?.items ?? [])
    .flatMap((item) => item.attachments).map((attachment) => [attachment.id, attachment])).values()]));
  const context = useBoard((s) => s.board?.conversationId === conversation.id ? s.board.context : null);
  const workers = useBoard((s) => s.board?.conversationId === conversation.id && Object.keys(s.board.tasks).length > 0);
  const empty = sources.length === 0 && attachments.length === 0 && !context && !workers;
  return <SummaryCard scrollKey={`${conversation.id}/context`} active={active}>
    <div className="flex flex-col py-2.5">
      {empty && <p className="text-muted-foreground px-4 py-1.5 text-sm">Nothing here yet</p>}
      <WorkersSummary conversationId={conversation.id} />
      <Sources conversationId={conversation.id} />
      {attachments.length > 0 && <SummarySection foldKey="attachments" title="Attachments" count={attachments.length}>
        {attachments.map((attachment) => <SummaryRow key={attachment.id}>{attachment.name}</SummaryRow>)}
      </SummarySection>}
      {context && <SummarySection foldKey="usage" title="Context usage">
        <SummaryRow>{context.usedTokens.toLocaleString()} tokens used</SummaryRow>
      </SummarySection>}
    </div>
  </SummaryCard>;
}

function SessionSummaryContent({
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
  const run = shownRun(overnight);
  const setup =
    conversation.setup?.type === "session" ? conversation.setup : null;
  const worktree = setup?.environment.type === "newWorktree";
  const sessionDiff = useSessionDiff(conversation.id, worktree && !run);
  const runDiff = useRunDiff(conversation.id, run?.id ?? null);
  // During and after a run the card shows the run's branch: that is where the work is.
  const diff = run ? runDiff : (sessionDiff?.stat ?? null);
  const pullRequest = usePullRequest(conversation.id);
  // Before a session has its folder and branch, its card is a chat's: never an empty space.
  if (!setup) return <ChatSummaryContent conversation={conversation} active={active} />;
  const checkout =
    setup.environment.type === "newWorktree"
      ? (setup.environment.path ?? setup.repo)
      : setup.repo;

  return (
    <>
      <SummaryCard scrollKey={`${conversation.id}/context`} active={active}>
        <div className="flex flex-col py-2.5">
          <SummarySection
            foldKey="project"
            title={<span title={checkout}>{project ?? setup.repo}</span>}
          >
            {/* Plain rows: the thread merges on the user's words, and the Source panel commits. */}
            <BranchRow branch={run?.workspace?.branch ?? setup.environment.branch} diff={diff} />
            {pullRequest && <PullRequestRow conversationId={conversation.id} pullRequest={pullRequest} />}
          </SummarySection>
          <WaitingOnYou conversationId={conversation.id} />
          <WorkersSummary conversationId={conversation.id} />
          <ActivitiesSummary conversationId={conversation.id} />
          <SessionSources conversationId={conversation.id} />
        </div>
      </SummaryCard>
      <JobStack conversationId={conversation.id} />
      {/* The run's card sits right under the job's. */}
      {overnight.map((model) => (
        <SummaryCard key={model.run.id} scrollKey={`${conversation.id}/overnight-${model.run.id}`} active={active}>
          <OvernightPlanCard model={model} actions={overnightActions} className={BARE} />
        </SummaryCard>
      ))}
    </>
  );
}

/** Something the session waits for at a set time: a usage limit's reset, a run's set end. */
type Wait = { key: string; title: string; reason?: string | undefined; atMs: number };

/** The overnight run states whose set end is still ahead. */
const RUN_ENDS = new Set(["running", "waitingQuota"]);

/**
 * What the session waits for at a set time, only where the board holds the time: the thread or
 * its workers paused until a usage limit resets, an overnight run's set end.
 */
function useWaits(conversationId: string): Wait[] {
  const lead = useApp((s) => s.conversations[conversationId]?.quotaWait ?? null);
  const json = useBoard((s) => {
    if (s.board?.conversationId !== conversationId) return "[]";
    const waits: Wait[] = [];
    if (lead?.resetsAtMs) waits.push({ key: "lead", title: `Resumes at ${formatTime(lead.resetsAtMs)}`, reason: lead.reason, atMs: lead.resetsAtMs });
    const paused = Object.values(s.board.tasks)
      .filter((task) => task.quotaWait?.resetsAtMs)
      .toSorted((a, b) => a.quotaWait!.resetsAtMs! - b.quotaWait!.resetsAtMs!);
    const first = paused[0];
    if (first && !lead) {
      const at = formatTime(first.quotaWait!.resetsAtMs!);
      const title = paused.length === 1 ? `${workerName(s.board.tasks, first)} resumes at ${at}` : `${paused.length} workers resume at ${at}`;
      waits.push({ key: "workers", title, reason: first.quotaWait!.reason, atMs: first.quotaWait!.resetsAtMs! });
    }
    for (const run of Object.values(s.board.overnight)) {
      if (run.windDownAtMs && RUN_ENDS.has(run.state)) {
        waits.push({ key: `run-${run.id}`, title: `The overnight run wraps up at ${formatTime(run.windDownAtMs)}`, reason: run.name, atMs: run.windDownAtMs });
      }
    }
    return JSON.stringify(waits);
  });
  return useMemo(() => JSON.parse(json) as Wait[], [json]);
}

/**
 * The session's job while it works through steps (Q88), then what it waits for at a set time:
 * cards of their own, under the context card or in its place while it is hidden.
 */
function JobStack({ conversationId }: { conversationId: string }) {
  const job = useSessionJobs(conversationId)?.current ?? null;
  const waits = useWaits(conversationId);
  const nowMs = useNow(waits.length > 0 ? 30_000 : null);
  return (
    <>
      {job && <JobCard job={job} onOpen={() => openPanelTab(conversationId, { type: "reviews" }, "Progress")} />}
      {waits.map((wait) => (
        <WaitCard key={wait.key} title={wait.title} reason={wait.reason} atMs={wait.atMs} nowMs={nowMs} />
      ))}
    </>
  );
}

/**
 * The top bar's summary button. Where the pane keeps the summary beside the thread it pins and
 * unpins it; where the summary floats, it opens it over the thread, under the top bar.
 */
export function PinnedSummaryToggle() {
  const pinned = useApp(summaryShownOf);
  const float = useSummary((s) => s.layout === "float");
  const floating = useSummary((s) => s.floating);
  if (!float) {
    return (
      <TooltipIconButton look="toolbar"
        tooltip="Context"
        aria-label="Context"
        aria-pressed={pinned}
        onClick={() => setPinnedSummary(!pinned)}
      >
        <Tasks />
      </TooltipIconButton>
    );
  }
  return (
    <PopoverTrigger asChild>
      <TooltipIconButton look="toolbar"
        tooltip="Context"
        aria-label="Context"
        aria-pressed={floating}
      >
        <Tasks />
      </TooltipIconButton>
    </PopoverTrigger>
  );
}
