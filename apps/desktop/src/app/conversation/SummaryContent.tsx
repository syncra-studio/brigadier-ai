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
import { rowName } from "@/app/conversation/activity/workerState";
import { gitCommandsKey, mcpKey, pullRequestKey, usedMcpServers, webUse } from "@/app/conversation/cliFacts";
import { ActivitiesSummary } from "@/app/conversation/Activities";
import { AllLinksPopover } from "@/app/conversation/AllLinksPopover";
import { BranchRow } from "@/app/conversation/BranchRow";
import { PULL_REQUEST_TONES, pullRequestLook } from "@/app/conversation/pullRequestWords";
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
import { openPanelTab } from "@/state/sessionTabs";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { WorkersSummary } from "@/app/side-panel/WorkerSummary";
import { TooltipIconButton } from "@/components/controls";
import { Popover, PopoverTrigger } from "@/components/overlays";
import { request } from "@/ipc/client";
import { openLink as openWebLink } from "@/state/links";
import type {
  Conversation,
  DiffStat,
  FileEntry,
  McpServerState,
  PullRequest,
  PullRequestState,
  WorkerRow,
  WorkerWait,
} from "@/ipc/generated";
import { getSessionDiff } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useCliThread, useSessionMcp, useWorkerRows } from "@/state/cliThread";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";
import { cn } from "@/lib/utils";

const NO_ENTRIES: readonly FileEntry[] = [];

/** A session's own CLI items, as its thread holds them. */
function useEntries(conversationId: string): readonly FileEntry[] {
  return useCliThread((s) => s.views[conversationId]?.entries ?? NO_ENTRIES);
}

/**
 * The branch's +N −N against its base, read again whenever a turn of its CLI ends and whenever a
 * git command of its thread finishes (a commit, a merge).
 */
function useSessionDiff(
  conversationId: string,
  worktree: boolean,
): { stat: DiffStat; commits: number } | null {
  const turns = useCliThread((s) => {
    const all = s.views[conversationId]?.turns ?? [];
    return `${all.length}:${all.at(-1)?.end ?? ""}`;
  });
  const entries = useEntries(conversationId);
  const git = useMemo(() => gitCommandsKey(entries), [entries]);
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
    // Each turn's end and each git command may move the branch: read it again.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [conversationId, worktree, turns, git]);
  return worktree ? stat : null;
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
  const entries = useEntries(conversationId);
  const servers = useSessionMcp(conversationId);
  return useMemo(() => {
    const users = (messages ?? []).filter((message) => message.role === "user");
    const files = [...new Map(users.flatMap((message) => message.attachments).map((file) => [file.id, file])).values()];
    const { pages } = webUse(entries);
    const used = usedMcpServers(entries);
    return [
      ...files.map((file): UsedSource => ({ key: `file:${file.id}`, icon: <Paperclip />, label: file.name, title: file.name })),
      ...pages.map((url): UsedSource => ({ key: `link:${url}`, icon: <Link />, label: hostOf(url), title: url, open: () => openLink(url) })),
      ...used.map((name): UsedSource => {
        const state = statusOf(servers, name);
        const label = serverLabel(state?.name ?? name);
        return { key: `mcp:${name}`, icon: <Plugin />, label, title: label, meta: state?.status === "needs-auth" ? "Needs sign-in" : undefined };
      }),
    ];
  }, [messages, entries, servers]);
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
  const entries = useEntries(conversationId);
  const searched = useMemo(() => webUse(entries).searches.length > 0, [entries]);
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

/**
 * The branch's GitHub pull request (from `gh`, read only): read when the card opens, and again
 * whenever a `git push` or a `gh pr` command of the thread finishes (`pullRequestKey`).
 */
function usePullRequest(conversationId: string): PullRequest | null {
  const entries = useEntries(conversationId);
  const pushed = useMemo(() => pullRequestKey(entries), [entries]);
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
    // A push or a `gh pr` command may have opened, moved or merged it: look again.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [conversationId, pushed]);
  return found;
}

/** The pull request row: `#N`, its state in a word and a colour (failed checks red); a click opens it. */
function PullRequestRow({ pullRequest }: { pullRequest: PullRequest }) {
  const look = pullRequestLook(pullRequest);
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
export function SummaryContent({ conversation, active, cards = null }: { conversation: Conversation | null; active: boolean; cards?: ReactNode }) {
  if (!conversation) return <SummaryCard scrollKey="draft/context" active={active}>
    <p className="text-muted-foreground p-4 text-sm">Nothing here yet</p>
  </SummaryCard>;
  return conversation.kind === "session" ? <SessionSummaryContent conversation={conversation} active={active} cards={cards} />
    : <ChatSummaryContent conversation={conversation} active={active} />;
}

/** Plain chats expose their own workers, sources, attachments and usage, without session sections. */
function ChatSummaryContent({ conversation, active }: { conversation: Conversation; active: boolean }) {
  const sources = useSources(conversation.id);
  const attachments = useApp(useShallow((s) => [...new Map((s.threads[conversation.id]?.items ?? [])
    .flatMap((item) => item.attachments).map((attachment) => [attachment.id, attachment])).values()]));
  const context = useBoard((s) => s.board?.conversationId === conversation.id ? s.board.context : null);
  const workers = useWorkerRows(conversation.id).length > 0;
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
  cards,
}: {
  conversation: Conversation;
  active: boolean;
  cards: ReactNode;
}) {
  const project = useApp((s) =>
    conversation.projectId
      ? (s.projects[conversation.projectId]?.name ?? null)
      : null,
  );
  const setup =
    conversation.setup?.type === "session" ? conversation.setup : null;
  const worktree = setup?.environment.type === "newWorktree";
  const sessionDiff = useSessionDiff(conversation.id, worktree);
  const diff = sessionDiff?.stat ?? null;
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
            <BranchRow branch={setup.environment.branch} diff={diff} />
            {pullRequest && <PullRequestRow pullRequest={pullRequest} />}
          </SummarySection>
          <WaitingOnYou conversationId={conversation.id} />
          <WorkersSummary conversationId={conversation.id} />
          <ActivitiesSummary conversationId={conversation.id} />
          <SessionSources conversationId={conversation.id} />
        </div>
      </SummaryCard>
      {cards}
    </>
  );
}
