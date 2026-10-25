import {
  type ReactNode,
  useEffect,
  useRef,
  useState,
} from "react";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";

import { pendingActionKeys, type PendingAction } from "@/app/conversation/pendingActions";
import { inField, useCardKeys } from "@/app/conversation/cardKeys";
import { PlanText, planTitle } from "@/app/conversation/cards/planCard";
import { QuestionAction } from "@/app/conversation/QuestionAction";
import { ReplyButton } from "@/app/conversation/ReplyButton";
import { StepCounter } from "@/app/conversation/StepCounter";
import { useAction } from "@/app/conversation/useAction";
import { WorkerChip } from "@/app/conversation/WorkerChip";
import { ActionCard, ActionCardActions, ActionCardTitle, ActionFileList, actionButton, ComposerRailItem, ToolBox, shownCommand } from "@/components/conversation";
import { Button, Kbd } from "@/components/controls";
import { Commit, Dock, Globe, Key, PencilSquare, QuestionMarkCircle, Sparkle } from "@/components/icons";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/overlays";
import type {
  ApprovalDecision,
  ApprovalRequest,
  Conversation,
  DiffStat,
} from "@/ipc/generated";
import { pageShown } from "@/lib/pageHidden";
import { cn } from "@/lib/utils";
import { answerCard } from "@/state/actions";
import { type ShownApproval, useBoard } from "@/state/board";
import { useApp } from "@/state/store";

/* Pending decisions sit on the rail above the full composer. Their shortcuts leave
 * text fields, focused controls and open overlays to handle their own keys. */

export type { PendingAction } from "@/app/conversation/pendingActions";

const NOTHING_ASIDE: readonly string[] = [];

/** The cards put aside with × (their ids), by conversation. */
const useAside = create<{
  byConversation: Readonly<Record<string, readonly string[]>>;
}>(() => ({
  byConversation: {},
}));

/** The conversation's cards put aside with ×. */
export function useAsideCards(
  conversationId: string | null,
): readonly string[] {
  return useAside(
    (s) =>
      (conversationId ? s.byConversation[conversationId] : undefined) ??
      NOTHING_ASIDE,
  );
}

export function setAsideCards(
  conversationId: string,
  ids: readonly string[],
): void {
  useAside.setState((s) => ({
    byConversation: { ...s.byConversation, [conversationId]: ids },
  }));
}

/** Brings a card put aside back to the rail ("Waiting on you" links to it). */
export function showCard(conversationId: string, cardId: string): void {
  const aside =
    useAside.getState().byConversation[conversationId] ?? NOTHING_ASIDE;
  setAsideCards(
    conversationId,
    aside.filter((id) => id !== cardId),
  );
}

/**
 * The decisions waiting for the user in a conversation, oldest first: approvals and open
 * questions.
 */
export function usePendingActions(
  conversation: Conversation | null,
): PendingAction[] {
  const keys = useBoard(
    useShallow((s) => pendingActionKeys(conversation, s.board)),
  );
  return keys.map((key) => {
    const [type, id] = key.split(":") as [PendingAction["type"], string];
    return { type, id };
  });
}

/** One pending decision on the rail, above the composer. */
export function PendingActionCard({
  action,
  more,
  onDismiss,
}: {
  action: PendingAction;
  /** How many more wait behind this one. */
  more: number;
  onDismiss: () => void;
}) {
  // A card that follows another (rather than the composer) fades its rows in.
  const [shown, setShown] = useState({ id: action.id, follows: false });
  if (shown.id !== action.id) setShown({ id: action.id, follows: true });
  const follows = shown.id !== action.id || shown.follows;
  const footer = more > 0 ? (
    <p className="text-foreground/50 px-4 pb-3 text-xs">
      {more} more {more === 1 ? "decision waits" : "decisions wait"} after this one
    </p>
  ) : null;
  return (
    <ComposerRailItem variant="decision" label="Waiting for you">{card()}</ComposerRailItem>
  );

  function card() {
    switch (action.type) {
      case "approval":
        return <ApprovalAction key={action.id} id={action.id} footer={footer} />;
      case "question":
        return (
          <QuestionAction
            key={action.id}
            id={action.id}
            onDismiss={onDismiss}
            footer={footer}
            stagger={follows}
          />
        );
    }
  }
}

// ----- approval ----------------------------------------------------------------------------

/** Shell-quotes an argument only where needed, so the exact argv reads unambiguously. */
function quote(arg: string): string {
  return /^[\w@%+=:,./-]+$/.test(arg)
    ? arg
    : `'${arg.replaceAll("'", `'\\''`)}'`;
}

type Shown = {
  icon: ReactNode;
  kind: string;
  title: ReactNode;
  body?: ReactNode;
  /** The primary button's words; "Allow once" by default. */
  allow?: string;
  /** The refusing button's words; "Deny" by default. */
  deny?: string;
  /** No refusing button, and Esc refuses nothing: the user replies instead (a plan). */
  noDeny?: boolean;
};

/** A file's line counts, at the end of its row. */
function fileStats(stat: DiffStat) {
  return stat.files.map((file) => ({
    path: file.path,
    trailing: (
      <span className="shrink-0 text-xs font-normal tabular-nums">
        {file.binary ? (
          <span className="text-foreground/50">binary</span>
        ) : (
          <>
            <span className="text-success">+{file.insertions}</span>{" "}
            <span className="text-destructive">−{file.deletions}</span>
          </>
        )}
      </span>
    ),
  }));
}

/** The host a network approval asks to reach, from its input (`{"host": "example.com"}`). */
function hostAsked(input: string | null): string | null {
  try {
    const value: unknown = JSON.parse(input ?? "");
    const host = value && typeof value === "object" ? (value as Record<string, unknown>)["host"] : null;
    return typeof host === "string" && host.trim() ? host.trim() : null;
  } catch {
    return null;
  }
}

/**
 * What a card asks, compactly: its kind ("Terminal", "Edit files", "Internet access", "Ask
 * permission", "Plan"), the asker's one-line reason as the title, and the exact thing asked
 * about. `cwd` shows only when the command runs outside the session's own folders.
 */
function describe(
  approval: ShownApproval,
  landingId: string | null,
  inSession: (path: string) => boolean,
): Shown {
  const { subject } = approval;
  switch (subject.type) {
    case "cli": {
      const { request } = subject;
      const edits = request.kind === "fileChange" || request.paths.length > 0;
      const web = request.tool === "SandboxNetworkAccess" || /web|fetch|search|network/i.test(request.tool);
      const [icon, kind] = request.command
        ? [<Dock key="icon" />, "Terminal"]
        : edits
          ? [<PencilSquare key="icon" />, "Edit files"]
          : web
            ? [<Globe key="icon" />, "Internet access"]
            : [<Key key="icon" />, "Ask permission"];
      const fallback = request.command
        ? "Allow this command?"
        : edits
          ? request.paths.length === 1
            ? "Allow editing this file?"
            : "Allow editing these files?"
          : web
            ? "Allow connecting to the internet?"
            : `Allow ${request.tool}?`;
      const cwd = request.cwd && !inSession(request.cwd) ? request.cwd : null;
      // A host the sandbox doesn't let the session reach: the question names it, nothing else to show.
      const host = !request.command && web ? hostAsked(request.input) : null;
      if (host) {
        return {
          icon,
          kind,
          title: request.reason || `Allow connecting to https://${host}?`,
          body: <p className="text-foreground/60 px-4 text-sm">{host} isn’t on this session’s list of allowed sites.</p>,
        };
      }
      return {
        icon,
        kind,
        // The asker's own justification, when it has one; else the plain question.
        title: request.reason || fallback,
        body: request.command ? (
          <ToolBox clamp className="mx-3">
            {shownCommand(request.command)}
            {cwd && <span className="text-foreground/35 block">in {cwd}</span>}
            {/* What the command asks for beyond its sandbox (`write /path`, `network`). */}
            {request.paths.length > 0 && (
              <span className="text-foreground/35 block">with {request.paths.join(", ")}</span>
            )}
          </ToolBox>
        ) : request.paths.length > 0 ? (
          <ActionFileList files={request.paths.map((path) => ({ path }))} />
        ) : (
          request.input && <ToolBox clamp className="mx-3">{request.input}</ToolBox>
        ),
      };
    }
    case "outwardCommand":
      return {
        icon: <Dock />,
        kind: "Terminal",
        title: "Allow this command? It reaches outside this computer.",
        body: <ToolBox clamp className="mx-3">{shownCommand(subject.argv.map(quote).join(" "))}</ToolBox>,
      };
    case "landing":
      return {
        icon: <Commit />,
        kind: "Land a change",
        title: landingId ? (
          <>
            Land <WorkerChip taskId={landingId} /> on {subject.branch}?
          </>
        ) : (
          `Land this change on ${subject.branch}?`
        ),
        body: <ActionFileList files={fileStats(subject.diffStat)} />,
        allow: "Land",
      };
    case "action":
      return {
        icon: <Key />,
        kind: "Ask permission",
        title: subject.action,
        body: subject.details && <ToolBox prose className="mx-3">{subject.details}</ToolBox>,
      };
    case "outline":
      return {
        icon: <Sparkle />,
        kind: "Plan",
        title: `Start this plan? ${subject.title}`,
        body: <ToolBox prose className="mx-3">{subject.outline}</ToolBox>,
        allow: "Yes, start it",
      };
    case "peerMessage":
      return {
        icon: <Sparkle />,
        kind: "Message",
        title: `Send a message to ${subject.title} in ${subject.project}?`,
        body: <ToolBox prose className="mx-3">{subject.text}</ToolBox>,
        allow: "Send",
        deny: "Don't send",
      };
    case "browser":
      return {
        icon: <Globe />,
        kind: "Browser",
        title: subject.question,
        body: <ToolBox prose className="mx-3">{subject.details}</ToolBox>,
        allow: "Go ahead",
        deny: "Don't",
      };
    // The plan the CLI proposed in its own plan mode: build it, or reply to change it (harness D7).
    case "plan":
      return {
        icon: <Sparkle />,
        kind: "Plan",
        title: planTitle(subject.plan),
        body: <PlanText plan={subject.plan} className="mx-3" />,
        allow: "Build it",
        noDeny: true,
      };
  }
}

/** Whether a folder is the session's own: its repository, or a worker's worktree or scratch folder. */
function useInSession(conversationId: string | undefined): (path: string) => boolean {
  const repo = useApp((s) => {
    const setup = conversationId ? s.conversations[conversationId]?.setup : undefined;
    return setup?.type === "session" ? setup.repo : null;
  });
  const roots = useBoard(
    useShallow((s) =>
      Object.values(s.board?.tasks ?? {}).flatMap((task) =>
        [task.workspace?.worktree, task.workspace?.scratch].filter((root): root is string => !!root),
      ),
    ),
  );
  return (path) => [repo, ...roots].some((root) => !!root && (path === root || path.startsWith(`${root}/`)));
}

/** Where a card stands among the approvals waiting: "1 of 3", with ‹ › to step between them. */
export type ApprovalStep = { index: number; total: number; onStep: (index: number) => void };

/**
 * An approval, compact: kind and asker, the reason as the title, the exact command, then
 * [Deny `Esc`] [Allow … for this session] [Allow once]. Esc denies from anywhere in the view;
 * allowing takes a click, or Enter on an Allow the user moved to: a stray Enter never allows.
 * A message to another session is the exception: like a question card, it takes focus, and
 * Enter on the focused card sends it (its ⏎ shows while it has focus).
 * Several waiting step "1 of 3" as each is answered. In the composer's place, "Reply…" brings
 * the message field back.
 */
export function ApprovalAction({
  id,
  footer,
  step,
  onReply,
}: {
  id: string;
  footer?: ReactNode;
  step?: ApprovalStep;
  onReply?: () => void;
}) {
  const approval = useBoard((s) => s.board?.approvals[id]);
  const actorId = useBoard((s) =>
    approval?.taskId && s.board?.tasks[approval.taskId]
      ? approval.taskId
      : null,
  );
  const landingId = useBoard((s) =>
    approval?.subject.type === "landing" &&
    s.board?.tasks[approval.subject.taskId]
      ? approval.subject.taskId
      : null,
  );
  const inSession = useInSession(approval?.conversationId);
  const action = useAction();
  const answer = (decision: ApprovalDecision) => {
    if (!approval || action.busy) return;
    action.run(() =>
      answerCard(approval.conversationId, approval.id, decision),
    );
  };
  const deny = () => answer({ type: "deny", message: "" });
  const peer = approval?.subject.type === "peerMessage";
  const plan = approval?.subject.type === "plan";
  const card = useRef<HTMLElement>(null);
  useEffect(() => {
    if (peer && !inField(document.activeElement)) card.current?.focus();
  }, [peer]);
  useCardKeys(
    approval !== undefined && !action.busy && !plan,
    peer ? { enter: () => answer({ type: "allow" }), escape: deny } : { escape: deny },
    card,
  );
  if (!approval) return null;

  const shown = describe(approval, landingId, inSession);
  const request =
    approval.subject.type === "cli" ? approval.subject.request : null;
  const grant = request ? sessionGrant(request) : null;
  return (
    <ActionCard
      ref={card}
      tabIndex={peer ? 0 : undefined}
      aria-label="Approval"
      data-action="approval"
      className="group/approval"
    >
      <div className="flex min-w-0 flex-col gap-2 px-4 pt-3.5 pb-2.5">
        <div className="text-foreground/65 text-code flex min-w-0 items-center gap-2 leading-5 [&_svg]:size-4.5 [&_svg]:shrink-0">
          {shown.icon}
          <span className="shrink-0">{shown.kind}</span>
          {actorId !== null && <WorkerChip taskId={actorId} className="min-w-0" />}
          {step && step.total > 1 && (
            <StepCounter
              data-slot="approval-step"
              noun="request"
              at={step.index}
              count={step.total}
              canBack={step.index > 0}
              canForward={step.index < step.total - 1}
              onStep={(by) => step.onStep(step.index + by)}
              className="ms-auto"
            />
          )}
        </div>
        <div role="alert" aria-atomic="true">
          <ActionCardTitle>{shown.title}</ActionCardTitle>
        </div>
      </div>
      {shown.body}
      <ActionCardActions
        className="flex-wrap justify-end"
        leading={
          <>
            {onReply && (
              <ReplyButton onClick={onReply} />
            )}
            {action.error && (
              <span role="alert" className="text-destructive me-auto text-xs">
                {action.error}
              </span>
            )}
          </>
        }
      >
        {!shown.noDeny && (
          <button
            type="button"
            className={actionButton(
              "outline",
              "@max-md/approval-card:justify-center",
            )}
            disabled={action.busy}
            onClick={deny}
          >
            {shown.deny ?? "Deny"}
            <Kbd variant="chip" aria-hidden className="group-hover/action:bg-current/15 @max-md/approval-card:hidden">
              Esc
            </Kbd>
          </button>
        )}
        {grant && (
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                aria-label={`Allow ${grant.scope} for this session`}
                className={actionButton("outline", "min-w-0 max-w-full @max-md/approval-card:justify-center")}
                disabled={action.busy}
                onClick={() => answer({ type: "allowSimilar" })}
              >
                <span className="flex min-w-0 items-center">
                  <span className="shrink-0">Allow&nbsp;</span>
                  <span className="truncate">{grant.scope}</span>
                  <span className="shrink-0">&nbsp;for this session</span>
                </span>
              </button>
            </TooltipTrigger>
            <TooltipContent>{grant.explanation}</TooltipContent>
          </Tooltip>
        )}
        <button
          type="button"
          className={actionButton("primary", "max-w-full @max-md/approval-card:justify-center")}
          disabled={action.busy}
          onClick={() => answer({ type: "allow" })}
        >
          <span className="truncate">{shown.allow ?? "Allow once"}</span>
          {peer && (
            <span className="hidden group-focus-within/approval:contents">
              <Kbd variant="chip" aria-hidden className="@max-md/approval-card:hidden">
                ⏎
              </Kbd>
            </span>
          )}
        </button>
      </ActionCardActions>
      {footer}
    </ActionCard>
  );
}

/**
 * The approvals waiting, one at a time in the composer's place, as the composer card: answering
 * one shows the next. "Reply…" brings the composer back with the card above it.
 */
export function ApprovalSlot({ ids, onReply }: { ids: readonly string[]; onReply: () => void }) {
  const [index, setIndex] = useState(0);
  // One that came while nobody could see the window is simply there when it shows again.
  const [opens] = useState(pageShown);
  const at = Math.min(index, ids.length - 1);
  const id = ids[at];
  if (id === undefined) return null;
  return (
    <div
      data-slot="approval-slot"
      className={cn("bg-composer rounded-composer shadow-hairline relative w-full backdrop-blur-lg", opens && "animate-rail-open motion-reduce:animate-none")}
    >
      <ApprovalAction
        key={id}
        id={id}
        step={{ index: at, total: ids.length, onStep: setIndex }}
        onReply={onReply}
      />
    </div>
  );
}

/** The scope supplied by approval routing; generic permission requests have no session grant. */
function sessionGrant(request: ApprovalRequest): { scope: string; explanation: string } | null {
  const { grant, kind, tool, escalation } = request;
  if (!grant || kind === "permissions") return null;
  const outside = escalation ? " outside the sandbox" : "";
  if (tool === "SandboxNetworkAccess") {
    return {
      scope: grant,
      explanation: `Allow connections to ${grant}${outside} from any worker in this conversation. Not saved across sessions.`,
    };
  }
  if (kind === "fileChange") {
    return {
      scope: "edits in this folder",
      explanation: `Allow file changes inside ${grant}${outside} from any worker in this conversation working in this folder. Edits outside this folder, in Git metadata or in CLI config (.claude, .codex, .agents, .vscode, .mcp.json) still ask. Not saved across sessions.`,
    };
  }
  if (kind === "command") {
    return {
      scope: grant,
      explanation: `Allow commands that start with ${grant}${outside} from any worker in this conversation. Not saved across sessions.`,
    };
  }
  return null;
}

/** What a card put aside with × leaves on the rail, so it stays one click away. */
export function WaitingReminder({
  count,
  onShow,
}: {
  count: number;
  onShow: () => void;
}) {
  return (
    <div className="text-muted-foreground min-h-row flex items-center gap-2 px-3 text-sm">
      <QuestionMarkCircle className="size-icon-sm shrink-0" />
      <span className="min-w-0 flex-1 truncate">
        {count === 1
          ? "A decision waits for you"
          : `${count} decisions wait for you`}
      </span>
      <Button size="xs" variant="ghost" onClick={onShow}>
        Show
      </Button>
    </div>
  );
}
