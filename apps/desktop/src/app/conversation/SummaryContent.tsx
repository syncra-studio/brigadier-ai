import {
  AttachmentIcon,
  ImageContextMenu,
  ImagePreview,
  inlineImageLabel,
  type OpenPastedText,
  pastedTextLabel,
  PastedTextOpenerContext,
  useAttachmentUrl,
  useAui,
  SummaryRow,
  SummaryRowButton,
  SummarySection,
  type ViewerImage,
} from "@/components/conversation";
import {
  type ReactNode,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useReducer,
  useState,
} from "react";
import { useShallow } from "zustand/react/shallow";

import { showCard } from "@/app/conversation/ActionCards";
import { rowName } from "@/app/conversation/activity/workerState";
import { gitCommandsKey, mcpKey, usedMcpServers, webUse } from "@/app/conversation/cliFacts";
import { ActivitiesSummary } from "@/app/conversation/Activities";
import { AllLinksPopover } from "@/app/conversation/AllLinksPopover";
import { BranchRow } from "@/app/conversation/BranchRow";
import { folderSharers } from "@/app/conversation/forks";
import { PULL_REQUEST_TONES, pullRequestLook } from "@/app/conversation/pullRequestWords";
import { activePullRequests, activeWaits, completeWait, useTrackedWork, waitingDue } from "@/app/conversation/trackedWork";
import type { PullRequestStatus } from "@/app/conversation/usePullRequest";
import { SourceRow } from "@/app/conversation/SourceRow";
import { hostOf, linksIn, openLink } from "@/app/conversation/summaryLinks";
import { useKeptScroll } from "@/app/conversation/summaryScroll";
import {
  Folder,
  HandRaised,
  Link,
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
import { openArtifact } from "@/ipc/client";
import { getSessionDiff, openConversation } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useCliThread, useSessionMcp, useThreadFact, useWorkerRows } from "@/state/cliThread";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";
import { heldMap } from "@/lib/held";
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
  const [, redraw] = useReducer((n: number) => n + 1, 0);
  useEffect(() => {
    if (!session) return;
    let current = true;
    getSessionDiff(conversationId)
      .then((next) => {
        lastDiffs.set(conversationId, next ?? undefined);
        if (current) redraw();
      })
      // The last stat read stays while git can't tell; the card shows the branch alone when none was.
      .catch(() => {});
    return () => {
      current = false;
    };
    // Each turn's end and each git command may move the branch: read it again.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [conversationId, session, turns, git]);
  // The last stat read of it shows while it is read again, opened anew too: never a blank between.
  return session ? (lastDiffs.get(conversationId) ?? null) : null;
}

/** Each session's last diff stat read, held across restarts so a first open shows it at once. */
const lastDiffs = heldMap(
  "brigadier.sessionDiffs",
  200,
  (v): v is { stat: DiffStat; commits: number } =>
    typeof v === "object" && v !== null && "stat" in v && typeof v.stat === "object" && v.stat !== null && Array.isArray((v.stat as DiffStat).files),
);

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
export type UsedSource = { key: string; icon: ReactNode; label: string; title: string; meta?: string | undefined; open?: () => void; viewer?: RowViewer };

/** An image row's place in the conversation's images: a click opens the full-window viewer on it. */
export type RowViewer = { file: AttachmentRef; label: string; gallery: { images: readonly ViewerImage[]; index: number } };

/**
 * An MCP server's name as a line reads it ("linear" reads "Linear"). A server known only by its
 * tool calls' names may carry its vendor's prefix there, which goes.
 */
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
      ...attachmentSources(files, (n, paste) => void openPastedTab(conversationId, n, paste)),
      ...pages.map((url): UsedSource => ({ key: `link:${url}`, icon: <Link />, label: hostOf(url), title: url, open: () => openLink(url) })),
      ...used.map((name): UsedSource => {
        const state = statusOf(servers, name);
        const label = serverLabel(state ? (state.label ?? state.name) : name);
        return { key: `mcp:${name}`, icon: <Plugin />, label, title: label, meta: state?.status === "needs-auth" ? "Needs sign-in" : undefined };
      }),
    ];
  }, [conversationId, messages, pagesSeen, serversUsed, servers]);
}

/**
 * What the default app would run instead of showing, or can't be read as text: apps, installers,
 * compiled scripts and the links that start something. A blob is copied without an execute bit,
 * but these launch without one or ask to.
 */
const LAUNCHES = new Set([
  "app", "command", "tool", "terminal", "workflow", "wflow", "action", "pkg", "mpkg", "dmg", "scpt", "scptd", "osascript", "osax",
  "prefpane", "saver", "qlgenerator", "kext", "mobileconfig", "shortcut", "widget", "plugin", "bundle", "webloc", "inetloc", "fileloc", "jar", "pyc",
  "exe", "com", "msi", "msix", "scr", "pif", "vbe", "jse", "wsf", "wsh", "hta", "lnk", "reg", "cpl", "url",
  "appimage", "desktop", "run", "deb", "rpm",
]);

/** Source text that runs when launched: the reader shows it and never runs it, whatever its type says (browsers often give none). */
const SCRIPTS = new Set([
  "sh", "bash", "zsh", "csh", "tcsh", "ksh", "fish", "py", "pyw", "rb", "pl", "php", "lua", "tcl", "js", "mjs", "cjs", "ps1", "bat", "cmd", "vbs", "applescript",
]);

function extensionOf(name: string): string {
  const bare = name.replace(/[.\s]+$/, "");
  const dot = bare.lastIndexOf(".");
  return dot < 0 ? "" : bare.slice(dot + 1).toLowerCase();
}

/** The extension of the copy the daemon writes for the default app: it keeps the name's first 80 characters, which can leave another extension last. */
function writtenExtension(name: string): string {
  return extensionOf([...name.replace(/^\.+/, "")].slice(0, 80).join(""));
}

/** What a click on an attachment's row does: the image viewer, the read-only reader, the default app, or it refuses. */
export function attachmentOpens(file: AttachmentRef): "viewer" | "reader" | "app" | "refused" {
  if (file.mime.startsWith("image/")) return "viewer";
  if (file.pasted) return "reader";
  const extension = extensionOf(file.name);
  const written = writtenExtension(file.name);
  if (LAUNCHES.has(extension) || LAUNCHES.has(written)) return "refused";
  if (SCRIPTS.has(extension) || file.mime.startsWith("text/") || file.mime === "application/json") return "reader";
  // Named otherwise, a copy cut down to a script's name still must not reach the default app.
  return SCRIPTS.has(written) ? "refused" : "app";
}

/** An attachment's name as its message reads it. */
function attachmentName(file: AttachmentRef): string {
  if (file.pasted) return pastedTextLabel(file.inline ?? null);
  return file.inline != null && file.mime.startsWith("image/") ? inlineImageLabel(file.inline) : file.name;
}

/** Attachments' row names, by id. Numbers restart per message, so a name already taken gets " (2)" after it. */
export function attachmentLabels(files: readonly AttachmentRef[]): Map<string, string> {
  const taken = new Set<string>();
  return new Map(files.map((file) => {
    const name = attachmentName(file);
    let label = name;
    for (let copy = 2; taken.has(label); copy += 1) label = `${name} (${copy})`;
    taken.add(label);
    return [file.id, label];
  }));
}

function refuse(file: AttachmentRef): void {
  toast(`${file.name} wasn't opened: it could run a program on this Mac.`, { tone: "error" });
}

function openInDefaultApp(file: AttachmentRef): void {
  openArtifact(file.id, file.name).catch((error: unknown) =>
    toast(error instanceof Error ? error.message : String(error), { tone: "error" }),
  );
}

/**
 * The conversation's attachments as rows, the same in a session's Sources, its Sources tab and a
 * Chat's Attachments. `openText` shows a text in the read-only reader, where the conversation has one.
 */
export function attachmentSources(files: readonly AttachmentRef[], openText: OpenPastedText | null): UsedSource[] {
  const labels = attachmentLabels(files);
  const images = files.filter((file) => attachmentOpens(file) === "viewer");
  const gallery = images.map((file): ViewerImage => ({ name: labels.get(file.id)!, source: { ref: file } }));
  return files.map((file): UsedSource => {
    const label = labels.get(file.id)!;
    const row: UsedSource = { key: `file:${file.id}`, icon: <AttachmentRowIcon file={file} />, label, title: file.name };
    switch (attachmentOpens(file)) {
      case "viewer":
        return { ...row, viewer: { file, label, gallery: { images: gallery, index: images.indexOf(file) } } };
      case "reader":
        return openText ? { ...row, open: () => openText(file.inline ?? null, file) } : row;
      case "app":
        return { ...row, open: () => openInDefaultApp(file) };
      case "refused":
        return { ...row, open: () => refuse(file) };
    }
  });
}

/** An attachment's row icon: a tiny thumbnail of an image, else its kind's icon. */
function AttachmentRowIcon({ file }: { file: AttachmentRef }) {
  const url = useAttachmentUrl(file.mime.startsWith("image/") ? { ref: file } : {});
  const [broken, setBroken] = useState(false);
  return url && !broken
    ? <img src={url} alt="" draggable={false} onError={() => setBroken(true)} className="size-icon-md rounded-xs object-cover" />
    : <AttachmentIcon mime={file.mime} />;
}

/** The viewer around an image row's button, which takes its click and ref. */
export function ImageRowViewer({ viewer: { file, label, gallery }, children }: { viewer: RowViewer; children: ReactNode }) {
  const url = useAttachmentUrl({ ref: file });
  return (
    <ImagePreview url={url} name={label} gallery={gallery}>
      <ImageContextMenu image={{ name: label, url, source: { ref: file } }}>{children}</ImageContextMenu>
    </ImagePreview>
  );
}

/** One of what a session used, or a Chat's attachment, on the card. */
function UsedRow({ source }: { source: UsedSource }) {
  const { title, icon, meta, label, open, viewer } = source;
  // Its own slot, over the triggers' around it.
  if (viewer) return <ImageRowViewer viewer={viewer}><SummaryRowButton data-slot="summary-row" title={title} icon={icon} meta={meta}>{label}</SummaryRowButton></ImageRowViewer>;
  return open
    ? <SummaryRowButton title={title} icon={icon} meta={meta} onClick={open}>{label}</SummaryRowButton>
    : <SummaryRow title={title} icon={icon} meta={meta}>{label}</SummaryRow>;
}

/** A Chat's "Attachments": the files attached to its messages. */
export function ChatAttachments({ attachments }: { attachments: readonly AttachmentRef[] }) {
  const openPasted = useContext(PastedTextOpenerContext);
  const sources = useMemo(() => attachmentSources(attachments, openPasted), [attachments, openPasted]);
  return <SummarySection foldKey="attachments" title="Attachments" count={attachments.length}>
    {sources.map((source) => <UsedRow key={source.key} source={source} />)}
  </SummarySection>;
}

/** Sources shown before "View all". */
const SOURCES = 3;

/** A worker's wait in a line: its question, or the server it wants a sign-in for. */
function waitLine(wait: WorkerWait): string {
  switch (wait.kind) {
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
 * "Waiting on you": the real pending questions and MCP sign-ins of this
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
      {used.slice(0, SOURCES).map((source) => <UsedRow key={source.key} source={source} />)}
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
  const workers = useWorkerRows(conversation.id).length > 0;
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

/** "Shares this folder with ‹session›" for each open fork or source working in this session's folder (D211). */
function SharedFolderRows({ conversation }: { conversation: Conversation }) {
  const sharers = useApp(useShallow((s) => folderSharers(conversation, s.conversations)));
  return sharers.map((other) => (
    <SummaryRowButton key={other.id} data-row="shared-folder" title={other.title} icon={<Folder />} onClick={() => openConversation(other.id)}>
      Shares this folder with {other.title}
    </SummaryRowButton>
  ));
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
            <SharedFolderRows conversation={conversation} />

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
