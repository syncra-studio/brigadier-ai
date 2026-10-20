import {
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  type RefObject,
  useContext,
  useEffect,
  useEffectEvent,
  useId,
  useRef,
  useState,
} from "react";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";

import { pendingActionKeys, type PendingAction } from "@/app/conversation/pendingActions";
import { CodeSpans } from "@/app/conversation/cards/codeSpans";
import { PlanCardLink } from "@/app/conversation/cards/PlanCardLink";
import { questionRound, uncommittedCard } from "@/app/conversation/cards/questionRound";
import { StepCounter } from "@/app/conversation/StepCounter";
import { revealOvernight } from "@/app/conversation/summaryState";
import { useAction } from "@/app/conversation/useAction";
import { ViewContext } from "@/app/conversation/viewContext";
import { WorkerChip } from "@/app/conversation/WorkerChip";
import {
  ActionCard,
  ActionCardActions,
  ActionCardQuestion,
  ActionCardTitle,
  ActionFileList,
  ActionFreeText,
  ActionOption,
  actionButton,
  staggered,
} from "@/components/assistant-ui/elements/action-card";
import { ComposerRailItem } from "@/components/assistant-ui/elements/composer-rail";
import { ToolBox } from "@/components/assistant-ui/elements/tool-box";
import { Button } from "@/components/ui/button";
import { Commit, Dock, Globe, Key, Mic, PencilSquare, QuestionMarkCircle, Reply, Sparkle, Spinner } from "@/components/icons";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import type {
  AnswerInput,
  ApprovalDecision,
  ApprovalRequest,
  Conversation,
  DiffStat,
} from "@/ipc/generated";
import { shownCommand } from "@/components/transcript/activity";
import { answerCard, answerQuestion, decidePlan, putOffPlan } from "@/state/actions";
import { type ShownApproval, useBoard } from "@/state/board";
import { type PlanDocRef, planDoc } from "@/state/planDoc";
import { useApp } from "@/state/store";
import {
  cancelDictation,
  dictationOwner,
  registerInserter,
  startDictation,
  stopDictation,
  useDictation,
} from "@/state/dictation";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Kbd } from "@/components/ui/kbd";

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

/** Where the user is on an open question card: what a visit to Settings must not lose. */
type KeptQuestion = {
  page: number;
  answers: readonly (string | null)[];
  /** Text typed on a question but not sent yet. */
  drafts: readonly (string | undefined)[];
  /** The answer lit on each question; -1 is the free-text row, null none. */
  lit: readonly (number | null | undefined)[];
};

const FRESH: KeptQuestion = { page: 0, answers: [], drafts: [], lit: [] };

/** Open question cards' state, by card id, until each is answered. */
const useKept = create<{ byCard: Readonly<Record<string, KeptQuestion>> }>(() => ({
  byCard: {},
}));

function keepQuestion(id: string, change: Partial<KeptQuestion>): void {
  useKept.setState((s) => ({
    byCard: { ...s.byCard, [id]: { ...(s.byCard[id] ?? FRESH), ...change } },
  }));
}

function forgetQuestion(id: string): void {
  useKept.setState((s) => {
    const byCard = { ...s.byCard };
    delete byCard[id];
    return { byCard };
  });
}

// A question answered elsewhere, withdrawn or dismissed takes its kept state with it.
useBoard.subscribe(({ board }, { board: before }) => {
  if (!board || board.conversationId !== before?.conversationId) return;
  const { byCard } = useKept.getState();
  for (const id of Object.keys(byCard)) {
    const question = board.questions[id];
    if (before.questions[id] && (!question || question.answer !== null || question.answeredAtMs !== null)) {
      forgetQuestion(id);
    }
  }
});

/** What an answer to a skipped question says, so the asker carries on. */
const SKIPPED = "Skipped: use your best judgment.";

/**
 * The decisions waiting for the user in a conversation, oldest first: approvals, open
 * questions, and plans a session under "Ask for approval" waits on.
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

/** A field the user types in; its keys are its own, not the card's. */
const FIELD =
  "input, textarea, select, [contenteditable]:not([contenteditable='false' i])";

/** Open menus, dialogs and the message field's popovers, which take Enter and Esc first. */
const OVERLAYS =
  "[role=dialog], [role=menu], [role=listbox], [data-slot=composer-commands], [data-slot=composer-mentions]";

function inField(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(FIELD) !== null;
}

/**
 * Esc puts the card aside, and Enter answers it, from anywhere in its view: not from a text
 * field (the composer sends), not Enter on a focused button or link (that clicks it), and not
 * while a menu or dialog is open (they take their keys first). A side chat's keys are its own.
 * Without `enter` Enter is left alone, and with `within` it answers only from inside that
 * element (the focused card).
 */
function useCardKeys(
  enabled: boolean,
  keys: { enter?: () => void; escape: () => void },
  within?: RefObject<HTMLElement | null>,
) {
  const { embedded } = useContext(ViewContext);
  const onKeyDown = useEffectEvent((event: globalThis.KeyboardEvent) => {
    if (event.key !== "Enter" && event.key !== "Escape") return;
    if (event.defaultPrevented || event.isComposing || event.repeat) return;
    if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey)
      return;
    const target = event.target instanceof Element ? event.target : null;
    if ((target?.closest("[data-embedded-view]") != null) !== embedded) return;
    if (inField(target)) return;
    if (event.key === "Enter") {
      if (!keys.enter) return;
      if (target?.closest("button, a[href], [role=button], summary")) return;
      if (within && !(target && within.current?.contains(target))) return;
    }
    if (document.querySelector(OVERLAYS)) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Enter") keys.enter?.();
    else keys.escape();
  });
  useEffect(() => {
    if (!enabled) return;
    const listener = (event: globalThis.KeyboardEvent) => onKeyDown(event);
    // Capture, so the card answers before the composer's Esc-to-stop sees the key.
    window.addEventListener("keydown", listener, true);
    return () => window.removeEventListener("keydown", listener, true);
  }, [enabled]);
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
      case "plan":
        return (
          <PlanAction
            key={action.id}
            id={action.id}
            onDismiss={onDismiss}
            footer={footer}
            stagger={follows}
          />
        );
      case "outline":
        return (
          <ImplementPlanAction
            key={action.id}
            docRef={{ type: "outline", id: action.id }}
            onDismiss={onDismiss}
            footer={footer}
            stagger={follows}
          />
        );
      case "overnight":
        return (
          <OvernightAction
            key={action.id}
            id={action.id}
            onDismiss={onDismiss}
            footer={footer}
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
  const card = useRef<HTMLElement>(null);
  useEffect(() => {
    if (peer && !inField(document.activeElement)) card.current?.focus();
  }, [peer]);
  useCardKeys(
    approval !== undefined && !action.busy,
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
              <button
                type="button"
                data-slot="approval-reply"
                className="text-foreground/55 hover:text-foreground hover:bg-foreground/8 text-code rounded-capsule h-control-sm inline-flex shrink-0 items-center gap-1.5 px-2 transition-colors"
                onClick={onReply}
              >
                <Reply className="size-icon-sm" />
                Reply…
              </button>
            )}
            {action.error && (
              <span role="alert" className="text-destructive me-auto text-xs">
                {action.error}
              </span>
            )}
          </>
        }
      >
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
  const at = Math.min(index, ids.length - 1);
  const id = ids[at];
  if (id === undefined) return null;
  return (
    <div
      data-slot="approval-slot"
      className="bg-composer rounded-composer shadow-hairline animate-rail-open relative w-full backdrop-blur-lg motion-reduce:animate-none"
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

// ----- question and plan -------------------------------------------------------------------

/** How long a picked answer shows its dot before it goes. */
const COMMIT_MS = 180;

/** A button's click as the user gave it: by the pointer, or by Enter or Space on it. */
function clickInput(event: MouseEvent): AnswerInput {
  return event.detail === 0 ? "enter" : "click";
}

/**
 * The card shared by questions and "Implement this plan?": numbered answers (1–9 light, ↑/↓
 * move, Enter picks the lit one; a pick shows its dot, then goes), then the free-text row
 * with Skip, which turns into Submit once something is typed (Next, before a round's last
 * question; ←/→ page through the round). Esc puts it aside; ×, with `onClose`, does that.
 *
 * Enter picks the lit answer only from the focused card, never from elsewhere in the window: a
 * click in the thread and Enter answers nothing. Every answer may change code or be hard to undo
 * (a merge, building a plan), so nothing is lit until the user moves to an answer, even when the
 * card took focus on its own: ↑/↓ or 1–9 light one (the pointer passing over lights nothing),
 * and the recommended answer is only marked. A click on an answer picks it.
 */
/**
 * The free-text row's microphone: dictates into the row, as the composer's does into the
 * composer; it stops (the words going into the row) or cancels while busy.
 */
function CardDictate({ owner, disabled }: { owner: string; disabled: boolean }) {
  const { available, phase } = useDictation(owner);
  if (!available) return null;
  const recording = phase.type === "recording";
  const busy = phase.type === "downloading" || phase.type === "starting" || phase.type === "transcribing";
  return (
    <TooltipIconButton
      tooltip={recording ? "Stop dictation" : busy ? "Cancel dictation" : "Dictate"}
      side="top"
      type="button"
      size="icon-md"
      data-card-dictate={phase.type}
      className="rounded-capsule"
      disabled={disabled || phase.type === "transcribing"}
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => (recording ? void stopDictation() : busy ? cancelDictation() : void startDictation(owner))}
    >
      {busy ? <Spinner className="size-icon-sm animate-spin motion-reduce:animate-none" /> : <Mic className={recording ? "size-icon-sm text-destructive" : "size-icon-sm"} />}
    </TooltipIconButton>
  );
}

function ChoiceCard({
  name,
  title,
  kind,
  detail,
  extra,
  choices,
  kept,
  initialText = "",
  placeholder,
  pager,
  last = true,
  onPage,
  onChoose,
  onText,
  onTyped,
  onLit,
  onSkip,
  skipLabel = "Skip",
  onDismiss,
  onClose,
  busy,
  error,
  footer,
  stagger,
}: {
  name: "question" | "plan";
  title: ReactNode;
  /** A few words over the question (a round's title). */
  kind?: ReactNode;
  detail?: ReactNode;
  /** Between the question and the answers (the files asked about). */
  extra?: ReactNode;
  choices: { label: string; description?: ReactNode; recommended?: boolean }[];
  /** The answer lit when the card was last shown, before the user went elsewhere. */
  kept?: number | null | undefined;
  /** What the free-text row holds at first (an answer typed before paging back). */
  initialText?: string;
  placeholder: string;
  /** A round's "‹ 1 of 3 ›", beside the dismiss button. */
  pager?: ReactNode;
  /** The round's last question: its button submits the round. */
  last?: boolean;
  /** ←/→ to the round's previous or next question. */
  onPage?: (step: -1 | 1) => void;
  /** An answer picked, and how: a click or Enter. */
  onChoose: (index: number, input: AnswerInput) => void;
  /** Words typed in the free-text row. */
  onText: (text: string) => void;
  /** What the free-text row holds as it is typed, so paging away and back keeps it. */
  onTyped?: (text: string) => void;
  /** The answer lit as it changes, so a visit elsewhere keeps it. */
  onLit?: (index: number | null) => void;
  onSkip: (input: AnswerInput) => void;
  /** What the button reads while nothing is typed. */
  skipLabel?: string;
  onDismiss: () => void;
  /** What × does instead of putting the card aside, and its label. */
  onClose?: { label: string; run: () => void };
  busy: boolean;
  error: string | null;
  footer: ReactNode;
  /** Fade the rows in one after another. */
  stagger: boolean;
}) {
  // The lit answer; -1 is the free-text row, null none yet.
  const [highlight, setHighlight] = useState<number | null>(
    choices.length === 0 ? -1 : (kept ?? null),
  );
  const lit = useEffectEvent((index: number | null) => onLit?.(index));
  useEffect(() => lit(highlight), [highlight]);
  const [chosen, setChosen] = useState<number | null>(null);
  const [typed, setTyped] = useState(initialText);
  // Dictated words go after what is typed, into this card alone: each mounted card (one per
  // question of a round, in each chat open) owns its own dictation.
  const owner = `card:${name}:${useId()}`;
  const dictated = useEffectEvent((words: string) => {
    const next = typed.trim() ? `${typed.trimEnd()} ${words}` : words;
    setTyped(next);
    onTyped?.(next);
    setHighlight(-1);
  });
  useEffect(() => {
    const unregister = registerInserter(owner, async (words) => dictated(words));
    return () => {
      unregister();
      // Paged away or answered while dictating: the words go nowhere rather than into another card.
      if (dictationOwner() === owner) cancelDictation();
    };
  }, [owner]);
  const card = useRef<HTMLElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  // The card takes focus (the field, when there is nothing to pick), unless the user is
  // typing somewhere.
  const pickable = choices.length > 0;
  useEffect(() => {
    if (!inField(document.activeElement))
      (pickable ? card : field).current?.focus();
  }, [pickable]);
  useEffect(() => () => clearTimeout(timer.current), []);

  const idle = !busy && chosen === null;
  const commit = (index: number, input: AnswerInput) => {
    if (!idle || choices[index] === undefined) return;
    setChosen(index);
    setHighlight(index);
    timer.current = setTimeout(() => {
      setChosen(null);
      onChoose(index, input);
    }, COMMIT_MS);
  };
  const text = typed.trim();
  const send = (input: AnswerInput) => {
    if (!idle) return;
    if (text) onText(text);
    else onSkip(input);
  };
  useCardKeys(
    idle,
    {
      enter: () => {
        if (highlight !== null && highlight >= 0) commit(highlight, "enter");
      },
      escape: onDismiss,
    },
    card,
  );
  const onKeyDown = (event: KeyboardEvent) => {
    if (
      !idle ||
      inField(event.target) ||
      event.metaKey ||
      event.ctrlKey ||
      event.altKey
    )
      return;
    const digit = Number(event.key);
    const lastChoice = choices.length - 1;
    if (
      Number.isInteger(digit) &&
      digit >= 1 &&
      digit <= Math.min(9, choices.length)
    ) {
      event.preventDefault();
      setHighlight(digit - 1);
    } else if (digit === choices.length + 1 && digit <= 9) {
      event.preventDefault();
      field.current?.focus();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      if (highlight === null) setHighlight(0);
      else if (highlight >= lastChoice) field.current?.focus();
      else setHighlight(highlight + 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (highlight === null || highlight === -1) setHighlight(lastChoice);
      else if (highlight > 0) setHighlight(highlight - 1);
    } else if (onPage && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
      event.preventDefault();
      onPage(event.key === "ArrowLeft" ? -1 : 1);
    }
  };

  const row = (index: number) => staggered(stagger, index);
  return (
    <ActionCard
      ref={card}
      container="request"
      tabIndex={0}
      aria-label={name === "plan" ? "Implement this plan?" : "Question"}
      data-action={name}
      onKeyDown={onKeyDown}
    >
      <div {...row(0)}>
        <ActionCardQuestion
          onDismiss={() => {
            if (!onClose) onDismiss();
            else if (idle) onClose.run();
          }}
          dismissLabel={onClose?.label}
          kind={kind}
          detail={detail}
          aside={pager}
        >
          {title}
        </ActionCardQuestion>
      </div>
      <div className="flex flex-col gap-3 pt-1 pb-2">
        {extra && <div {...row(1)}>{extra}</div>}
        <div className="flex flex-col gap-1 px-2">
          {choices.length > 0 && (
            <div
              role="radiogroup"
              aria-label="Answers"
              className="flex flex-col gap-1"
            >
              {choices.map((choice, index) => (
                <ActionOption
                  key={choice.label}
                  role="radio"
                  aria-checked={index === highlight}
                  // "Yes, implement this plan" alone: an empty square.
                  number={name === "plan" ? null : index + 1}
                  label={choice.label}
                  description={choice.description ?? undefined}
                  recommended={choice.recommended}
                  highlighted={index === highlight}
                  chosen={index === chosen}
                  disabled={busy}
                  onClick={(event) => commit(index, clickInput(event))}
                  {...row(index + 2)}
                />
              ))}
            </div>
          )}
          <ActionFreeText
            ref={field}
            value={typed}
            aria-label={name === "plan" ? "What should change" : "Your answer"}
            placeholder={placeholder}
            marker={choices.length > 0}
            highlighted={choices.length > 0 && highlight === -1}
            disabled={busy}
            onFocus={() => setHighlight(-1)}
            onChange={(event) => {
              setTyped(event.target.value);
              onTyped?.(event.target.value);
              if (event.target.value) setHighlight(-1);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                // Never the composer's form around the card.
                event.preventDefault();
                if (text) send("enter");
              } else if (event.key === "ArrowUp" && choices.length > 0) {
                event.preventDefault();
                setHighlight(choices.length - 1);
                card.current?.focus();
              } else if (event.key === "Escape") {
                // Back to the answers; a second Esc puts the card aside.
                event.preventDefault();
                card.current?.focus();
              }
            }}
            {...row(choices.length + 2)}
          >
            <CardDictate owner={owner} disabled={busy} />
            <button
              type="button"
              className={actionButton(
                text ? "primary" : "outline",
                "font-medium",
              )}
              disabled={busy}
              onClick={(event) => send(clickInput(event))}
            >
              {!text ? skipLabel : last ? "Submit" : "Next"}
            </button>
          </ActionFreeText>
        </div>
      </div>
      {error && (
        <p role="alert" className="text-destructive px-4 pb-2 text-xs">
          {error}
        </p>
      )}
      {footer}
    </ActionCard>
  );
}

/**
 * A question card from Brigadier, the thread or a worker: its round, one question at a time with
 * a "‹ 1 of 3 ›" pager, and every answer sent at once after the last. Skip tells the asker to
 * use its judgment.
 */
function QuestionAction({
  id,
  onDismiss,
  footer,
  stagger,
}: {
  id: string;
  onDismiss: () => void;
  footer: ReactNode;
  stagger: boolean;
}) {
  const question = useBoard((s) => s.board?.questions[id]);
  const askerId = useBoard((s) =>
    question?.taskId && s.board?.tasks[question.taskId]
      ? question.taskId
      : null,
  );
  const action = useAction();
  // Kept by card id, not in the card: opening Settings unmounts it, and coming back finds the
  // round's answers, the text typed and the answer lit as they were.
  const { page, answers, drafts, lit } = useKept((s) => s.byCard[id] ?? FRESH);
  if (!question) return null;

  const round = questionRound(question);
  const at = Math.min(page, round.length - 1);
  const item = round[at]!;
  const uncommitted = uncommittedCard(question);
  // Back to any question answered so far, or on to the first one still open.
  const reachable = (to: number) =>
    to >= 0 && to < round.length && to <= answers.filter((answer) => answer !== null).length;
  const setPage = (to: number) => keepQuestion(id, { page: to });
  const draft = (text: string | undefined) =>
    keepQuestion(id, { drafts: round.map((_, index) => (index === at ? text : useKept.getState().byCard[id]?.drafts[index])) });
  // A merge card's Skip is its "Not yet": words typed there are a new request instead.
  const skipped = (each: (typeof round)[number]) =>
    question.kind.type === "merge" ? (each.options.at(-1)?.label ?? SKIPPED) : SKIPPED;
  const answer = (text: string, input: AnswerInput) => {
    const next = round.map((_, index) => (index === at ? text : (answers[index] ?? null)));
    keepQuestion(id, { answers: next });
    draft(undefined);
    const open = next.findIndex((given) => given === null);
    if (open >= 0) {
      setPage(open);
      return;
    }
    action.run(() =>
      answerQuestion(question.conversationId, question.id, next.map((given) => given ?? SKIPPED), input).then(() =>
        forgetQuestion(id),
      ),
    );
  };
  const given = answers[at] ?? null;
  const picked = given === null ? -1 : item.options.findIndex((option) => option.label === given);
  const pager =
    round.length > 1 ? (
      <StepCounter
        data-slot="round-pager"
        noun="question"
        at={at}
        count={round.length}
        canBack={reachable(at - 1)}
        canForward={reachable(at + 1)}
        onStep={(step) => setPage(at + step)}
        className="-mt-0.5 h-icon-button-sm"
      />
    ) : undefined;
  return (
    <ChoiceCard
      // A question of the round starts fresh, but keeps an answer given, or text typed, before
      // paging away.
      key={at}
      name="question"
      title={
        uncommitted
          ? uncommitted.title
          : <CodeSpans text={item.text} />
      }
      kind={question.title}
      detail={
        uncommitted ? (
          "Brigadier asks once, before a worker starts from them. They are never committed either way."
        ) : askerId === null ? undefined : (
          <>
            <WorkerChip taskId={askerId} /> waits for this
          </>
        )
      }
      extra={
        uncommitted &&
        uncommitted.files.length > 0 && (
          <ActionFileList
            files={uncommitted.files.map(({ path, threads }) => ({
              path,
              trailing: threads && (
                <span className="text-muted-foreground shrink-0 text-xs">the thread's</span>
              ),
            }))}
          />
        )
      }
      choices={item.options.map((option, index) => ({
        label: option.label,
        description: option.description && <CodeSpans text={option.description} />,
        recommended: item.recommended === index,
      }))}
      kept={lit[at] ?? (picked >= 0 ? picked : null)}
      initialText={drafts[at] ?? (given !== null && picked < 0 && given !== SKIPPED ? given : "")}
      placeholder={
        item.options.length > 0
          ? "No, and tell Brigadier what to do differently"
          : "Type here"
      }
      pager={pager}
      last={answers.filter((other, index) => index !== at && other !== null).length === round.length - 1}
      onPage={(step) => reachable(at + step) && setPage(at + step)}
      onChoose={(index, input) => answer(item.options[index]?.label ?? "", input)}
      onText={(text) => answer(text, "typed")}
      onTyped={draft}
      onLit={(index) =>
        keepQuestion(id, { lit: round.map((_, other) => (other === at ? index : useKept.getState().byCard[id]?.lit[other])) })
      }
      onSkip={(input) => answer(skipped(item), input)}
      onDismiss={onDismiss}
      // × skips what is still open in the round, so the asker hears and carries on (issue #239).
      onClose={{
        label: "Skip",
        run: () =>
          action.run(() =>
            answerQuestion(
              question.conversationId,
              question.id,
              round.map((each, index) => answers[index] ?? skipped(each)),
              "click",
            ).then(() => forgetQuestion(id)),
          ),
      }}
      busy={action.busy}
      error={action.error}
      footer={footer}
      stagger={stagger && at === 0}
    />
  );
}

/**
 * "Implement this plan?" for a plan the user reads as a document: Yes builds it; typed words
 * send it back. A thread's own plan is decided as a plan (the thread revises it); a lead's
 * outline answers its approval (Yes starts the lead, words go to it as corrections). A plan's
 * "Not now" puts it off, which the thread answers in a line; an outline's Skip, and ×, put the
 * card aside.
 */
function ImplementPlanAction({
  docRef,
  onDismiss,
  footer,
  stagger,
}: {
  docRef: PlanDocRef;
  onDismiss: () => void;
  footer: ReactNode;
  stagger: boolean;
}) {
  const conversationId = useBoard((s) => s.board?.conversationId ?? null);
  const shown = useBoard((s) => planDoc(s.board, docRef) !== null);
  const action = useAction();
  if (!shown || !conversationId) return null;
  const decide = (yes: boolean, text: string | null) =>
    action.run(() =>
      docRef.type === "plan"
        ? decidePlan(conversationId, docRef.id, yes, text)
        : answerCard(conversationId, docRef.id, yes ? { type: "allow" } : { type: "deny", message: text ?? "" }),
    );
  return (
    <ChoiceCard
      name="plan"
      title="Implement this plan?"
      choices={[{ label: "Yes, implement this plan" }]}
      placeholder="No, and tell Brigadier what to do differently"
      onChoose={() => decide(true, null)}
      onText={(text) => decide(false, text)}
      skipLabel={docRef.type === "plan" ? "Not now" : "Skip"}
      onSkip={docRef.type === "plan" ? () => action.run(() => putOffPlan(conversationId, docRef.id)) : onDismiss}
      onDismiss={onDismiss}
      busy={action.busy}
      error={action.error}
      footer={footer}
      stagger={stagger}
    />
  );
}

/**
 * A proposed plan: one written as a document asks "Implement this plan?"; one of phases alone
 * keeps its rail link to the plan card, with message entry available.
 */
function PlanAction({
  id,
  onDismiss,
  footer,
  stagger,
}: {
  id: string;
  onDismiss: () => void;
  footer: ReactNode;
  stagger: boolean;
}) {
  const document = useBoard((s) => !!s.board?.plans[id]?.body);
  return document ? (
    <ImplementPlanAction docRef={{ type: "plan", id }} onDismiss={onDismiss} footer={footer} stagger={stagger} />
  ) : (
    <PhasePlanAction id={id} onDismiss={onDismiss} footer={footer} />
  );
}

/**
 * The rail points to the single plan card and keeps message entry available. The plan is
 * approved on its card, never by an Enter on the rail: Esc puts it off for later.
 */
function PhasePlanAction({
  id,
  onDismiss,
  footer,
}: {
  id: string;
  onDismiss: () => void;
  footer: ReactNode;
}) {
  const plan = useBoard((s) => s.board?.plans[id]);
  const action = useAction();
  useCardKeys(Boolean(plan) && !action.busy, { escape: onDismiss });
  return (
    <div className="flex flex-col gap-2">
      <div className="min-h-row flex min-w-0 items-center gap-2 px-3">
        <div className="min-w-0 flex-1">
          <PlanCardLink cardId={id} />
        </div>
        <Button type="button" size="xs" variant="ghost" className="shrink-0" onClick={onDismiss}>
          Later
        </Button>
      </div>
      {action.error && (
        <p role="alert" className="text-destructive px-3 text-xs">
          {action.error}
        </p>
      )}
      {footer}
    </div>
  );
}

/** An overnight proposal: the rail points to its card, where Start is. */
function OvernightAction({
  id,
  onDismiss,
  footer,
}: {
  id: string;
  onDismiss: () => void;
  footer: ReactNode;
}) {
  const run = useBoard((s) => s.board?.overnight[id]);
  useCardKeys(Boolean(run), {
    enter: () => revealOvernight(id),
    escape: onDismiss,
  });
  if (!run) return null;
  return (
    <div className="flex flex-col gap-2">
      <div className="min-h-row flex min-w-0 items-center gap-2 px-3">
        <Button
          type="button"
          variant="link"
          size="sm"
          className="h-auto min-w-0 flex-1 justify-start px-0 text-start"
          onClick={() => revealOvernight(id)}
        >
          <span className="min-w-0 truncate" title={run.name}>
            View plan: {run.name}
          </span>
        </Button>
        <Button type="button" size="xs" variant="ghost" className="shrink-0" onClick={onDismiss}>
          Later
        </Button>
      </div>
      {footer}
    </div>
  );
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
