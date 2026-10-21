import { useAui, SummaryRow, SummaryRowButton, SummarySection } from "@/components/conversation";
import {
  type ReactNode,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
} from "react";
import { useShallow } from "zustand/react/shallow";

import { showCard } from "@/app/conversation/ActionCards";
import { useAskRequest } from "@/app/conversation/activity/askRequest";
import { revertPullRequest } from "@/app/conversation/activity/revert";
import { connectorName } from "@/app/conversation/activity/words";
import { isRunRequest } from "@/app/conversation/blocks";
import { shownRun, useOvernightCards, useRunDiff } from "@/app/conversation/overnightAdapter";
import { ActivitiesSummary } from "@/app/conversation/Activities";
import { AllLinksPopover } from "@/app/conversation/AllLinksPopover";
import { BranchRow } from "@/app/conversation/BranchRow";
import { JobStack } from "@/app/conversation/JobStack";
import { SourceRow } from "@/app/conversation/SourceRow";
import { workerName } from "@/app/conversation/rowWords";
import { hostOf, linksIn, openLink } from "@/app/conversation/summaryLinks";
import { useKeptScroll } from "@/app/conversation/summaryScroll";
import {
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
} from "@/components/icons";
import { openPanelTab } from "@/state/sessionTabs";
import { AgentsPanelContext, WorkerLine } from "@/app/conversation/WorkerChip";
import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { WorkersSummary } from "@/app/side-panel/WorkerSummary";
import { TooltipIconButton, Button } from "@/components/controls";
import { Popover, PopoverTrigger } from "@/components/overlays";
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
import { getSessionDiff } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

const NO_TASKS: Readonly<Record<string, Task>> = {};
const NO_WAITING: readonly WaitingItem[] = [];

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
        <AllLinksPopover sources={sources} />
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
 * The summary's cards, pinned in the pane or floating from the top bar: the context card, the
 * job's cards, then the run's card. `active` while they can be seen, for their scroll offsets.
 */
export function SummaryContent({ conversation, active }: { conversation: Conversation | null; active: boolean }) {
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
    </>
  );
}
