import { type OpenPastedText, PastedTextOpenerContext, useAui, SummaryRow, SummaryRowButton, SummarySection } from "@/components/conversation";
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
import { rowName } from "@/app/conversation/activity/workerState";
import { gitCommandsKey, mcpKey, usedMcpServers, webUse } from "@/app/conversation/cliFacts";
import { ActivitiesSummary } from "@/app/conversation/Activities";
import { AllLinksPopover } from "@/app/conversation/AllLinksPopover";
import { BranchRow } from "@/app/conversation/BranchRow";
import { PULL_REQUEST_TONES, pullRequestLook } from "@/app/conversation/pullRequestWords";
import { activePullRequests, activeWaits, completeWait, useTrackedWork, waitingDue } from "@/app/conversation/trackedWork";
import type { PullRequestStatus } from "@/app/conversation/usePullRequest";
import { SourceRow } from "@/app/conversation/SourceRow";
import { hostOf, linksIn, openLink } from "@/app/conversation/summaryLinks";
import { useKeptScroll } from "@/app/conversation/summaryScroll";
import {
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
import { openPanelTab, openPastedTab } from "@/state/sessionTabs";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { WorkersSummary } from "@/app/side-panel/WorkerSummary";
import { TooltipIconButton } from "@/components/controls";
import { Popover, PopoverTrigger } from "@/components/overlays";
import { openLink as openWebLink } from "@/state/links";
import type {
  AttachmentRef,
  Conversation,
  DiffStat,
  FileEntry,
  McpServerState,
  PullRequest,
  PullRequestState,
  TrackedWork,
  WorkerRow,
  WorkerWait,
} from "@/ipc/generated";
import { getSessionDiff } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useCliThread, useSessionMcp, useThreadFact, useWorkerRows } from "@/state/cliThread";
import { isNativeWorker } from "@/state/nativeWorkers";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";
import { cn } from "@/lib/utils";

/* What the summary reads from a session's own CLI items, each a string or a flag (see `useThreadFact`). */
const pagesOf = (entries: readonly FileEntry[]) => webUse(entries).pages.join("\n");
const serversOf = (entries: readonly FileEntry[]) => usedMcpServers(entries).join("\n");
const searchedIn = (entries: readonly FileEntry[]) => webUse(entries).searches.length > 0;

/**
 * A worktree session's +N −N against its base, a Local checkout's uncommitted changes, read
 * again whenever a turn of its CLI ends and whenever a git command of its thread finishes (a
 * commit, a merge).
 */
function useSessionDiff(
  conversationId: string,
  session: boolean,
): { stat: DiffStat; commits: number } | null {
  const turns = useCliThread((s) => {
    const all = s.views[conversationId]?.turns ?? [];
    return `${all.length}:${all.at(-1)?.end ?? ""}`;
  });
  const git = useThreadFact(conversationId, gitCommandsKey);
  const [stat, setStat] = useState<{ stat: DiffStat; commits: number } | null>(null);
  useEffect(() => {
    if (!session) return;
    let current = true;
    getSessionDiff(conversationId)
      .then((next) => current && setStat(next))
      // The card shows the branch alone when git can't tell.
      .catch(() => current && setStat(null));
    return () => {
      current = false;
    };
    // Each turn's end and each git command may move the branch: read it again.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [conversationId, session, turns, git]);
  return session ? stat : null;
}

/** A Chat's sources: links in the user's messages (as soon as they are sent), first seen first. */
function useSources(conversationId: string): string[] {
  const messages = useApp((s) => s.threads[conversationId]?.items);
  return useMemo(() => {
    const said = (messages ?? []).flatMap((message) =>
      message.role === "user" ? linksIn(message.text) : [],
    );
    return [...new Set(said)];
  }, [messages]);
}

/** One of what a session used, as its card lists it. */
export type UsedSource = { key: string; icon: ReactNode; label: string; title: string; meta?: string | undefined; open?: () => void };

/** An MCP server's name as a line reads it: `claude_ai_Linear` reads "Linear". */
export function serverLabel(name: string): string {
  const words = name.replace(/^claude[._ ]ai[._ ]/i, "").replace(/[_-]+/g, " ").trim();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : name;
}

/** A used server's status as the CLI said it, by its name in the CLI's list. */
function statusOf(servers: readonly McpServerState[], used: string): McpServerState | undefined {
  const key = mcpKey(used);
  return servers.find((server) => mcpKey(server.name) === key);
}

/**
 * What a session used, as its card's Sources list it (S14): the files attached to the user's
 * messages, then the pages its CLI fetched or browsed, then the MCP servers it called, each
 * with its status as the CLI last said it (a sign-in it wants). Its searches are in the Sources
 * tab.
 */
export function useUsedSources(conversationId: string): UsedSource[] {
  const messages = useApp((s) => s.threads[conversationId]?.items);
  const pagesSeen = useThreadFact(conversationId, pagesOf);
  const serversUsed = useThreadFact(conversationId, serversOf);
  const servers = useSessionMcp(conversationId);
  return useMemo(() => {
    const users = (messages ?? []).filter((message) => message.role === "user");
    const files = [...new Map(users.flatMap((message) => message.attachments).map((file) => [file.id, file])).values()];
    const pages = pagesSeen === "" ? [] : pagesSeen.split("\n");
    const used = serversUsed === "" ? [] : serversUsed.split("\n");
    return [
      ...files.map((file) => fileSource(conversationId, file)),
      ...pages.map((url): UsedSource => ({ key: `link:${url}`, icon: <Link />, label: hostOf(url), title: url, open: () => openLink(url) })),
      ...used.map((name): UsedSource => {
        const state = statusOf(servers, name);
        const label = serverLabel(state?.name ?? name);
        return { key: `mcp:${name}`, icon: <Plugin />, label, title: label, meta: state?.status === "needs-auth" ? "Needs sign-in" : undefined };
      }),
    ];
  }, [conversationId, messages, pagesSeen, serversUsed, servers]);
}

/** How a Chat's attachment row opens it: a pasted text over the thread, as its chip or card does; another file not at all. */
export function pastedOpener(open: OpenPastedText | null, file: AttachmentRef): (() => void) | undefined {
  return open && file.pasted ? () => open(file.inline ?? null, file) : undefined;
}

/** A Chat's "Attachments": the files attached to the user's messages; a pasted text opens over the thread. */
export function ChatAttachments({ attachments }: { attachments: readonly AttachmentRef[] }) {
  const openPasted = useContext(PastedTextOpenerContext);
  return <SummarySection foldKey="attachments" title="Attachments" count={attachments.length}>
    {attachments.map((attachment) => {
      const open = pastedOpener(openPasted, attachment);
      return open
        ? <SummaryRowButton key={attachment.id} onClick={open}>{attachment.name}</SummaryRowButton>
        : <SummaryRow key={attachment.id}>{attachment.name}</SummaryRow>;
    })}
  </SummarySection>;
}

/** A file attached to the user's messages, as Sources lists it: a pasted text opens in its tab, as its chip or card does. */
export function fileSource(conversationId: string, file: AttachmentRef): UsedSource {
  const open = file.pasted ? () => void openPastedTab(conversationId, file.inline ?? null, file) : undefined;
  return { key: `file:${file.id}`, icon: <Paperclip />, label: file.name, title: file.name, ...(open ? { open } : {}) };
}

/** Sources shown before "View all". */
const SOURCES = 3;

/** A worker's wait in a line: what it asks to run, its question, the server it wants a sign-in for. */
function waitLine(wait: WorkerWait): string {
  switch (wait.kind) {
    case "permission":
      return `Allow ${wait.what}`;
    case "question":
      return wait.what;
    case "signIn":
      return `Sign in to ${wait.what}`;
  }
}

/**
 * One thing a worker waits on the user for, on one line under the worker's name; its whole text
 * in its tooltip. A click opens the worker, and its card in the thread when it has one: the user
 * answers it here, and the session relays it (G19.Q6). It ends without a Done, when the worker's
 * CLI has its answer.
 */
function WaitingRow({ conversationId, row, name, wait }: { conversationId: string; row: WorkerRow; name: string; wait: WorkerWait }) {
  const { setPanel } = useContext(AgentsPanelContext);
  const line = waitLine(wait);
  const { card } = wait;
  return (
    <SummaryRowButton
      data-slot="waiting-row"
      data-wait={wait.kind}
      title={`${line}\nFrom ${name}`}
      icon={<HandRaised />}
      onClick={() => {
        setPanel(row.id);
        if (card) showCard(conversationId, card);
      }}
    >
      <span className="text-muted-foreground">{name}</span> {line}
    </SummaryRowButton>
  );
}

/**
 * "Waiting on you": the real pending permission prompts, questions and MCP sign-ins of this
 * session's workers, from their rows, the oldest worker's first. The section is there only while
 * something waits. The session's own question shows only above its composer.
 */
function WaitingOnYou({ conversationId }: { conversationId: string }) {
  const rows = useWorkerRows(conversationId);
  const items = rows.toReversed().flatMap((row) => row.waiting.map((wait, index) => ({ key: `${row.id}:${index}:${wait.kind}:${wait.what}`, row, wait })));
  if (items.length === 0) return null;
  return (
    <SummarySection foldKey="waiting" title="Waiting on you" count={items.length}>
      {items.map((item) => (
        <WaitingRow key={item.key} conversationId={conversationId} row={item.row} name={rowName(rows, item.row.id) ?? item.row.title} wait={item.wait} />
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
 * A session's "Sources": what it used (attachments, pages, MCP servers), the first three, then
 * "View all", which opens the Sources tab with every search, page and server.
 */
function SessionSources({ conversationId }: { conversationId: string }) {
  const used = useUsedSources(conversationId);
  const searched = useThreadFact(conversationId, searchedIn);
  if (used.length === 0) return null;
  return (
    <SummarySection foldKey="sources" title="Sources" count={used.length} action={<AddSource />}>
      {used.slice(0, SOURCES).map((source) =>
        source.open ? (
          <SummaryRowButton key={source.key} title={source.title} icon={source.icon} meta={source.meta} onClick={source.open}>
            {source.label}
          </SummaryRowButton>
        ) : (
          <SummaryRow key={source.key} title={source.title} icon={source.icon} meta={source.meta}>
            {source.label}
          </SummaryRow>
        ),
      )}
      {(used.length > SOURCES || searched) && (
        <SummaryRowButton muted icon={<Link />} onClick={() => openPanelTab(conversationId, { type: "sources" }, "Sources")}>
          View all
        </SummaryRowButton>
      )}
    </SummarySection>
  );
}

/** A Chat's "Sources": the links in the user's messages; "View all" lists every one. */
function Sources({ conversationId }: { conversationId: string }) {
  const sources = useSources(conversationId);
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

const PULL_REQUEST_ICONS: Record<PullRequestState, ReactNode> = {
  open: <PullRequestOpen />,
  draft: <PullRequestDraft />,
  merged: <PullRequestMerged />,
  closed: <PullRequestClosed />,
};

/** Watched PRs and named waits use the same compact rows as the rest of the card. */
function TrackedWorkSummary({ sessionId, fallback, authRequired }: { sessionId: string; fallback: PullRequest | null; authRequired?: boolean | undefined }) {
  const { work, error } = useTrackedWork(sessionId);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 30_000); return () => clearInterval(timer); }, []);
  return <TrackedWorkSections sessionId={sessionId} work={work} error={error} now={now} fallback={fallback} authRequired={authRequired} />;
}

export function TrackedWorkSections({ sessionId, work, error, now = 0, fallback, authRequired }: { sessionId: string; work: TrackedWork; error: string | null; now?: number; fallback: PullRequest | null; authRequired?: boolean | undefined }) {
  const prs = activePullRequests(work);
  const waits = activeWaits(work);
  const [completing, setCompleting] = useState<string | null>(null);
  const branch = fallback && fallback.state !== "merged" && fallback.state !== "closed" && !work.pullRequests.some((pr) => pr.identity.number === fallback.number && `https://${pr.identity.host}/${pr.identity.owner}/${pr.identity.repo}/pull/${pr.identity.number}`.toLowerCase() === fallback.url.toLowerCase()) ? fallback : null;
  return <>
    {(prs.length > 0 || branch || authRequired || work.prHistory.length > 0 || error) && <SummarySection foldKey="pull-requests" title="Pull requests" count={prs.length + (branch ? 1 : 0)}>
      {branch && <PullRequestRow pullRequest={branch} />}
      {prs.map((pr) => {
        const state = pr.pullRequest?.state ?? "open";
        const look = pr.complete && pr.pullRequest ? pullRequestLook(pr.pullRequest) : null;
        const tone = look ? PULL_REQUEST_TONES[look.tone] : "text-muted-foreground";
        const url = `https://${pr.identity.host}/${pr.identity.owner}/${pr.identity.repo}/pull/${pr.identity.number}`;
        return <SummaryRowButton key={url} data-slot="tracked-pr-row" title={`${pr.pullRequest?.title ?? url}\n${pr.actor} · ${pr.action}\n${pr.account ? `Account: ${pr.account}` : "Account needed"}`} icon={<span className={cn("flex", tone)}>{PULL_REQUEST_ICONS[state]}</span>} meta={<span className={cn("max-w-48 truncate", tone)}>{pr.status}</span>} onClick={() => openLink(url)}>{pr.identity.repo} #{pr.identity.number}</SummaryRowButton>;
      })}
      {authRequired && prs.length === 0 && <SummaryRow data-slot="pull-request-auth" muted icon={<PullRequestOpen />}>gh auth login for PR status</SummaryRow>}
      {error && <SummaryRow muted>Pull request status unavailable</SummaryRow>}
      {work.prHistory.length > 0 && <SummaryRowButton muted icon={<PullRequestClosed />} onClick={() => openPanelTab(sessionId, { type: "pr-history" }, "Pull request history")}>View history</SummaryRowButton>}
    </SummarySection>}
    {waits.length > 0 && <SummarySection foldKey="waiting-on" title="Waiting on" count={waits.length}>
      {waits.map((wait) => <div key={wait.id} className="flex min-w-0 items-center">
        {wait.link ? <SummaryRowButton className="min-w-0 flex-1" title={`${wait.then ?? wait.text}\n${wait.timezone}`} icon={<HandRaised />} meta={<span className={wait.dueAtMs !== null && wait.dueAtMs <= now ? "text-destructive" : undefined}>{waitingDue(wait, now)}</span>} onClick={() => openLink(wait.link!)}>{wait.text}</SummaryRowButton> : <SummaryRow className="min-w-0 flex-1" title={`${wait.then ?? wait.text}\n${wait.timezone}`} icon={<HandRaised />} meta={<span className={wait.dueAtMs !== null && wait.dueAtMs <= now ? "text-destructive" : undefined}>{waitingDue(wait, now)}</span>}>{wait.text}</SummaryRow>}
        <button className="text-muted-foreground hover:text-foreground focus-visible:text-foreground mr-3 shrink-0 px-1 py-1 text-xs disabled:opacity-50" disabled={completing === wait.id} aria-label={`Mark done: ${wait.text}`} onClick={() => {
          setCompleting(wait.id);
          void completeWait(sessionId, wait.id).catch((cause: unknown) => toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" })).finally(() => setCompleting(null));
        }}>Done</button>
      </div>)}
    </SummarySection>}
  </>;
}

/** The pull request row: `#N`, its state in a word and a colour (failed checks red); a click opens it. */
function PullRequestRow({ pullRequest }: { pullRequest: PullRequest }) {
  const observed = pullRequestLook(pullRequest);
  // Initial branch discovery has not fetched the complete canonical review/check pages.
  const look = observed.word === "Ready" ? { word: "Checking status", tone: "quiet" as const } : observed;
  const tone = PULL_REQUEST_TONES[look.tone];
  return (
    <SummaryRowButton
      data-slot="pull-request-row"
      data-tone={look.tone}
      title={`${pullRequest.title}\n${pullRequest.url}`}
      icon={<span className={cn("flex", tone)}>{PULL_REQUEST_ICONS[pullRequest.state]}</span>}
      meta={<span data-slot="pull-request-state" className={tone}>{look.word}</span>}
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
 * The summary's cards, pinned in the pane or floating from the top bar: the context card, then
 * the cards the conversation's view gives it to show under it (`cards`: the job's, "Resumes at").
 * `active` while they can be seen, for their scroll offsets.
 */
export function SummaryContent({ conversation, active, cards = null, prStatus }: { conversation: Conversation | null; active: boolean; cards?: ReactNode; prStatus: PullRequestStatus }) {
  if (!conversation) return <SummaryCard scrollKey="draft/context" active={active}>
    <p className="text-muted-foreground p-4 text-sm">Nothing here yet</p>
  </SummaryCard>;
  return conversation.kind === "session" ? <SessionSummaryContent conversation={conversation} active={active} cards={cards} prStatus={prStatus} />
    : <ChatSummaryContent conversation={conversation} active={active} />;
}

/** Plain chats expose their own workers, sources, attachments and usage, without session sections. */
function ChatSummaryContent({ conversation, active }: { conversation: Conversation; active: boolean }) {
  const sources = useSources(conversation.id);
  const attachments = useApp(useShallow((s) => [...new Map((s.threads[conversation.id]?.items ?? [])
    .flatMap((item) => item.attachments).map((attachment) => [attachment.id, attachment])).values()]));
  const context = useBoard((s) => s.board?.conversationId === conversation.id ? s.board.context : null);
  // Its CLI's subagents are not workers (`WorkersSummary`).
  const workers = useWorkerRows(conversation.id).some((row) => !isNativeWorker(row));
  const empty = sources.length === 0 && attachments.length === 0 && !context && !workers;
  return <SummaryCard scrollKey={`${conversation.id}/context`} active={active}>
    <div className="flex flex-col py-2.5">
      {empty && <p className="text-muted-foreground px-4 py-1.5 text-sm">Nothing here yet</p>}
      <WorkersSummary conversationId={conversation.id} />
      <Sources conversationId={conversation.id} />
      {attachments.length > 0 && <ChatAttachments attachments={attachments} />}
      {context && <SummarySection foldKey="usage" title="Context usage">
        <SummaryRow>{context.usedTokens.toLocaleString()} tokens used</SummaryRow>
      </SummarySection>}
    </div>
  </SummaryCard>;
}

function SessionSummaryContent({
  conversation,
  active,
  cards,
  prStatus,
}: {
  conversation: Conversation;
  active: boolean;
  cards: ReactNode;
  prStatus: PullRequestStatus;
}) {
  const project = useApp((s) =>
    conversation.projectId
      ? (s.projects[conversation.projectId]?.name ?? null)
      : null,
  );
  const setup =
    conversation.setup?.type === "session" ? conversation.setup : null;
  const sessionDiff = useSessionDiff(conversation.id, setup !== null);
  const diff = sessionDiff?.stat ?? null;
  const { pullRequest, authRequired } = prStatus;
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
            <BranchRow branch={setup.environment.branch} diff={diff} />

          </SummarySection>
          <WaitingOnYou conversationId={conversation.id} />
          <WorkersSummary conversationId={conversation.id} />
          <TrackedWorkSummary sessionId={conversation.id} fallback={pullRequest} authRequired={authRequired} />
          <ActivitiesSummary conversationId={conversation.id} />
          <SessionSources conversationId={conversation.id} />
        </div>
      </SummaryCard>
      {cards}
    </>
  );
}
