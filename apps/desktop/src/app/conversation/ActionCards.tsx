import {
  Branch,
  ChevronDown,
  Commit,
  Globe,
  InfoCircle,
  PencilSquare,
  QuestionMarkCircle,
  Sparkle,
  Terminal,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  type KeyboardEvent,
  type ReactNode,
  useContext,
  useEffect,
  useEffectEvent,
  useRef,
  useState,
} from "react";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";

import { pendingActionKeys, type PendingAction } from "@/app/conversation/pendingActions";
import { PlanCardLink } from "@/app/conversation/cards/PlanCardLink";
import { revealOvernight } from "@/app/conversation/summaryState";
import { useAction } from "@/app/conversation/useAction";
import { ViewContext } from "@/app/conversation/viewContext";
import { WorkerChip } from "@/app/conversation/WorkerChip";
import {
  ActionCard,
  ActionCardActions,
  ActionCardCode,
  ActionCardHeader,
  ActionCardQuestion,
  ActionFileList,
  ActionFreeText,
  ActionKbd,
  ActionOption,
  actionButton,
  staggered,
} from "@/components/assistant-ui/elements/action-card";
import { ComposerRailItem } from "@/components/assistant-ui/elements/composer-rail";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import type {
  Approval,
  ApprovalDecision,
  Conversation,
  DiffStat,
} from "@/ipc/generated";
import { NEVER_PUSHES_NOTE } from "@/lib/setup";
import { answerCard, answerQuestion, decidePlan } from "@/state/actions";
import { useBoard } from "@/state/board";

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
 * Enter and Esc answer the card from anywhere in its view: not from a text field (the
 * composer sends), not Enter on a focused button or link (that clicks it), and not while
 * a menu or dialog is open (they take their keys first). A side chat's keys are its own.
 */
function useCardKeys(
  enabled: boolean,
  keys: { enter: () => void; escape: () => void },
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
    if (
      event.key === "Enter" &&
      target?.closest("button, a[href], [role=button], summary")
    )
      return;
    if (document.querySelector(OVERLAYS)) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Enter") keys.enter();
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
    <ComposerRailItem label="Waiting for you">{card()}</ComposerRailItem>
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
  kind: ReactNode;
  title: ReactNode;
  detail?: ReactNode;
  /** Under the question (a warning). */
  note?: ReactNode;
  body?: ReactNode;
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

/** The kinds shown ("Terminal", "Edit files", "Internet access") for what is asked. */
function describe(
  approval: Approval,
  actorId: string | null,
  landingId: string | null,
): Shown {
  // The worker that asks, and the one to land, as their chips; Brigadier asks for itself.
  const actor = actorId === null ? null : <WorkerChip taskId={actorId} />;
  const who = actor ?? "Brigadier";
  const { subject } = approval;
  switch (subject.type) {
    case "cli": {
      const { request } = subject;
      const edits = request.kind === "fileChange" || request.paths.length > 0;
      const web = /web|fetch|search|network/i.test(request.tool);
      const [icon, kind] = request.command
        ? [<Terminal key="icon" />, "Terminal"]
        : edits
          ? [<PencilSquare key="icon" />, "Edit files"]
          : web
            ? [<Globe key="icon" />, "Internet access"]
            : [<Sparkle key="icon" />, request.tool];
      const question = request.command ? (
        actor ? (
          <>Do you want {actor} to run this command?</>
        ) : (
          "Allow Brigadier to run this command?"
        )
      ) : edits ? (
        <>
          Allow {who} to edit{" "}
          {request.paths.length === 0
            ? "files"
            : request.paths.length === 1
              ? "the following file"
              : "the following files"}
          ?
        </>
      ) : web ? (
        <>Allow {who} to connect to the internet?</>
      ) : (
        <>
          Allow {who} to use {request.tool}?
        </>
      );
      return {
        icon,
        kind,
        // The asker's own justification, when it has one; else the plain question.
        title: request.reason || question,
        // Who asks and where it runs, quietly under the question.
        detail:
          [
            request.reason && actor ? `Asked by ${actor}` : null,
            request.escalation ? "Runs outside the sandbox" : null,
          ]
            .filter(Boolean)
            .join(" · ") || undefined,
        body: request.command ? (
          <ActionCardCode>{request.command}</ActionCardCode>
        ) : request.paths.length > 0 ? (
          <ActionFileList files={request.paths.map((path) => ({ path }))} />
        ) : (
          request.input && <ActionCardCode>{request.input}</ActionCardCode>
        ),
      };
    }
    case "outwardCommand":
      return {
        icon: <Terminal />,
        kind: "Terminal",
        title: actor ? (
          <>Do you want {actor} to run a command that reaches outside?</>
        ) : (
          "Allow Brigadier to run a command that reaches outside?"
        ),
        detail: NEVER_PUSHES_NOTE,
        body: (
          <ActionCardCode>{subject.argv.map(quote).join(" ")}</ActionCardCode>
        ),
      };
    case "landing": {
      const task =
        landingId === null ? "this task" : <WorkerChip taskId={landingId} />;
      return {
        icon: <Commit />,
        kind: "Land a change",
        title: (
          <>
            Land {task} on {subject.branch}?
          </>
        ),
        detail: "One reviewed commit",
        body: <ActionFileList files={fileStats(subject.diffStat)} />,
      };
    }
    case "finishSession":
      return {
        icon: <Branch />,
        kind: "Finish session",
        title: `Merge ${subject.branch} into ${subject.base}?`,
        detail: `${subject.commits} commit${subject.commits === 1 ? "" : "s"}`,
        body: <ActionFileList files={fileStats(subject.diffStat)} />,
      };
    case "action":
      return {
        icon: <Sparkle />,
        kind: actor ?? "Action",
        title: subject.action,
        body: subject.details && (
          <p className="text-foreground/65 px-4 pb-2 text-sm whitespace-pre-wrap">
            {subject.details}
          </p>
        ),
      };
  }
}

/**
 * [Deny `Esc`] [Allow once `⏎`], Allow focused so Enter allows; both keys work from
 * anywhere in the view.
 */
function ApprovalAction({ id, footer }: { id: string; footer: ReactNode }) {
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
  const action = useAction();
  const allow = useRef<HTMLButtonElement>(null);
  // Unless the user is typing somewhere, Allow takes focus.
  useEffect(() => {
    // Focused for Enter, without a focus ring: it wasn't reached by keyboard.
    if (!inField(document.activeElement))
      allow.current?.focus({ focusVisible: false });
  }, []);
  const answer = (decision: ApprovalDecision) => {
    if (!approval || action.busy) return;
    action.run(() =>
      answerCard(approval.conversationId, approval.id, decision),
    );
  };
  const deny = () => answer({ type: "deny", message: "" });
  useCardKeys(approval !== undefined && !action.busy, {
    enter: () => answer({ type: "allow" }),
    escape: deny,
  });
  if (!approval) return null;

  const shown = describe(approval, actorId, landingId);
  const request =
    approval.subject.type === "cli" ? approval.subject.request : null;
  const grant = request?.grant ?? null;
  const allowLabel = (
    <>
      <span className="truncate">Allow once</span>
      <ActionKbd variant="primary">⏎</ActionKbd>
    </>
  );
  return (
    <ActionCard aria-label="Approval" data-action="approval">
      <ActionCardHeader
        icon={shown.icon}
        kind={shown.kind}
        title={shown.title}
        detail={shown.detail}
      >
        {shown.note}
      </ActionCardHeader>
      {shown.body}
      <ActionCardActions
        leading={
          action.error && (
            <span role="alert" className="text-destructive me-auto text-xs">
              {action.error}
            </span>
          )
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
          Deny
          <ActionKbd variant="outline">Esc</ActionKbd>
        </button>
        {grant ? (
          <div className="rounded-capsule inline-flex min-w-0 items-stretch self-start overflow-hidden @max-md/approval-card:w-full">
            <button
              ref={allow}
              type="button"
              className={actionButton(
                "primary",
                "min-w-0 rounded-e-none border-e-0 pe-1 focus-visible:ring-inset @max-md/approval-card:flex-1 @max-md/approval-card:justify-center @max-md/approval-card:ps-6",
              )}
              disabled={action.busy}
              onClick={() => answer({ type: "allow" })}
            >
              {allowLabel}
            </button>
            <GrantMenu
              command={grant}
              escalation={request?.escalation ?? false}
              disabled={action.busy}
              onAnswer={answer}
            />
          </div>
        ) : (
          <button
            ref={allow}
            type="button"
            className={actionButton(
              "primary",
              "max-w-full @max-md/approval-card:justify-center",
            )}
            disabled={action.busy}
            onClick={() => answer({ type: "allow" })}
          >
            {allowLabel}
          </button>
        )}
      </ActionCardActions>
      {footer}
    </ActionCard>
  );
}

/**
 * The ⌄ half of the split "Allow once" button. "Allow similar commands" becomes an exact
 * grant: this command, for the rest of this worker's CLI session, never saved.
 */
function GrantMenu({
  command,
  escalation,
  disabled,
  onAnswer,
}: {
  command: string;
  escalation: boolean;
  disabled: boolean;
  onAnswer: (decision: ApprovalDecision) => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label="Approval options"
          className={actionButton(
            "primary",
            "gap-0 rounded-s-none border-s-0 ps-0.5 pe-1.5 focus-visible:ring-inset",
          )}
          disabled={disabled}
        >
          <ChevronDown className="size-icon-sm opacity-50" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="end">
        <DropdownMenuItem onSelect={() => onAnswer({ type: "allow" })}>
          Allow once
        </DropdownMenuItem>
        <Tooltip>
          <TooltipTrigger asChild>
            <DropdownMenuItem
              onSelect={() => onAnswer({ type: "allowSimilar" })}
            >
              Don't ask again for this command
              <InfoCircle className="ms-auto opacity-75" />
            </DropdownMenuItem>
          </TooltipTrigger>
          <TooltipContent side="right">
            <span>
              Allow <code className="font-mono break-all">{command}</code>
              {escalation && " outside the sandbox"} again for this worker
            </span>
          </TooltipContent>
        </Tooltip>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// ----- question and plan -------------------------------------------------------------------

/** How long a picked answer shows its dot before it goes. */
const COMMIT_MS = 180;

/**
 * The card shared by questions and "Implement this plan?": numbered answers (1–9 pick, ↑/↓
 * move, Enter picks the lit one; a pick shows its dot, then goes), then the free-text row
 * with Skip, which turns into Submit once something is typed. Esc or × puts it aside.
 */
function ChoiceCard({
  name,
  title,
  detail,
  extra,
  choices,
  initial,
  placeholder,
  onChoose,
  onText,
  onSkip,
  onDismiss,
  busy,
  error,
  footer,
  stagger,
}: {
  name: "question" | "plan";
  title: ReactNode;
  detail?: ReactNode;
  /** Between the question and the answers (the files asked about). */
  extra?: ReactNode;
  choices: { label: string; recommended?: boolean }[];
  /** The answer lit at first. */
  initial: number;
  placeholder: string;
  onChoose: (index: number) => void;
  onText: (text: string) => void;
  onSkip: () => void;
  onDismiss: () => void;
  busy: boolean;
  error: string | null;
  footer: ReactNode;
  /** Fade the rows in one after another. */
  stagger: boolean;
}) {
  // The lit answer; -1 is the free-text row.
  const [highlight, setHighlight] = useState(choices.length > 0 ? initial : -1);
  const [chosen, setChosen] = useState<number | null>(null);
  const [typed, setTyped] = useState("");
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
  const commit = (index: number) => {
    if (!idle || choices[index] === undefined) return;
    setChosen(index);
    setHighlight(index);
    timer.current = setTimeout(() => {
      setChosen(null);
      onChoose(index);
    }, COMMIT_MS);
  };
  const text = typed.trim();
  const send = () => {
    if (!idle) return;
    if (text) onText(text);
    else onSkip();
  };
  useCardKeys(idle, {
    enter: () => {
      if (highlight >= 0) commit(highlight);
    },
    escape: onDismiss,
  });
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
    const last = choices.length - 1;
    if (
      Number.isInteger(digit) &&
      digit >= 1 &&
      digit <= Math.min(9, choices.length)
    ) {
      event.preventDefault();
      commit(digit - 1);
    } else if (digit === choices.length + 1 && digit <= 9) {
      event.preventDefault();
      field.current?.focus();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      if (highlight >= last) field.current?.focus();
      else setHighlight(highlight + 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (highlight === -1) setHighlight(last);
      else if (highlight > 0) setHighlight(highlight - 1);
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
        <ActionCardQuestion onDismiss={onDismiss} detail={detail}>
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
                  number={index + 1}
                  label={choice.label}
                  recommended={choice.recommended}
                  highlighted={index === highlight}
                  chosen={index === chosen}
                  disabled={busy}
                  onPointerEnter={() => idle && setHighlight(index)}
                  onClick={() => commit(index)}
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
              if (event.target.value) setHighlight(-1);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                // Never the composer's form around the card.
                event.preventDefault();
                if (text) send();
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
            <button
              type="button"
              className={actionButton(
                text ? "primary" : "outline",
                "font-medium",
              )}
              disabled={busy}
              onClick={send}
            >
              {text ? "Submit" : "Skip"}
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

/** A question from Brigadier or a worker; Skip tells the asker to use its judgment. */
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
  if (!question) return null;

  const uncommitted =
    question.kind.type === "uncommittedChanges" ? question.kind.files : null;
  const answer = (text: string) =>
    action.run(() =>
      answerQuestion(question.conversationId, question.id, text),
    );
  return (
    <ChoiceCard
      name="question"
      title={
        uncommitted
          ? "Should workers see your uncommitted changes?"
          : question.text
      }
      detail={
        uncommitted ? (
          "Brigadier asks once, before the first worker starts. They are never committed either way."
        ) : askerId === null ? undefined : (
          <>
            <WorkerChip taskId={askerId} /> waits for this
          </>
        )
      }
      extra={
        uncommitted &&
        uncommitted.length > 0 && (
          <ActionFileList files={uncommitted.map((path) => ({ path }))} />
        )
      }
      choices={question.options.map((label, index) => ({
        label,
        recommended: question.recommended === index,
      }))}
      initial={question.recommended ?? 0}
      placeholder={
        question.options.length > 0
          ? "No, and tell Brigadier what to do differently"
          : "Type here"
      }
      onChoose={(index) => answer(question.options[index] ?? "")}
      onText={answer}
      onSkip={() => answer(SKIPPED)}
      onDismiss={onDismiss}
      busy={action.busy}
      error={action.error}
      footer={footer}
      stagger={stagger}
    />
  );
}

/** The rail points to the single plan card and keeps message entry available. */
function PlanAction({
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
  // Preserve the existing rail shortcuts; focused controls keep their own Enter.
  useCardKeys(Boolean(plan) && !action.busy, {
    enter: () => {
      if (plan)
        action.run(() => decidePlan(plan.conversationId, plan.id, true, null));
    },
    escape: onDismiss,
  });
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
