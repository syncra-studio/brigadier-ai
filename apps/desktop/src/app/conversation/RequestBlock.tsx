import {
  ActionBarPrimitive,
  MessagePrimitive,
  useAuiState,
} from "@assistant-ui/react";
import {
  Branch,
  Check,
  ChevronRight,
  Copy,
  Regenerate,
  SoundOnReadOutLoudSpeaker,
  Stop,
  TextShorterConcise,
} from "@openai/apps-sdk-ui/components/Icon";
import { type FC, lazy, Suspense, useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { ForkMenu } from "@/app/conversation/ForkMenu";
import { MentionText } from "@/app/conversation/Mentions";
import { OrchestratorSteps, STEP_ROW } from "@/app/conversation/OrchestratorSteps";
import {
  type BlockCard,
  type BlockCompaction,
  type BlockOrchestratorStep,
  type BlockRow,
  type BlockState,
  blockSequence,
  isFinal,
  isLive,
  isRunRequest,
  isWorking,
  type SequenceEntry as Entry,
} from "@/app/conversation/blocks";
import { type PhaseView, phaseViewOf } from "@/app/conversation/phaseView";
import { PhaseChecksRow, TaskRow } from "@/app/conversation/TaskRow";
import { TurnDiff } from "@/app/conversation/TurnDiff";
import { TurnMemories } from "@/app/conversation/TurnMemories";
import { useViewConversation } from "@/app/conversation/viewContext";
import { WorkerMention } from "@/app/conversation/WorkerChip";
import {
  BranchPicker,
  MessageError,
  MessageText,
  StreamingMessageText,
} from "@/components/assistant-ui/thread";
import { preserveAnchor } from "@/components/assistant-ui/preserve-anchor";
import { RateItem, RateMenu } from "@/components/assistant-ui/rate-menu";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import type { ModelChoice } from "@/ipc/generated";
import { formatDuration, formatSentAt } from "@/lib/format";
import { modelName, sameModel, useModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { openConversation } from "@/state/actions";
import { readAloud, stopReading, useReading } from "@/state/readAloud";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const CardBody = lazy(() => import("@/app/conversation/cards/CardBody"));

/** What an assistant block carries in its message metadata (`custom.block`). */
export type BlockMeta = {
  /** Per text part, in order: the reply's position (infinite while it streams) and model. */
  texts: { position: number; model: ModelChoice | null }[];
  cards: BlockCard[];
  /** The workers' rows, in the order they started. */
  rows: BlockRow[];
  /** The orchestrator's steps, in order. */
  orchestratorSteps: BlockOrchestratorStep[];
  /** Messages the user steered into the turn, shown as bubbles in the work. */
  steers: { position: number; text: string; atMs: number }[];
  /** Compactions of a Chat's context in the turn, or after it. */
  compactions: BlockCompaction[];
  state: BlockState;
  startedAtMs: number;
  endedAtMs: number | null;
  /** The model the user picked, to flag a fallback. */
  picked: ModelChoice | null;
  /** A session's block always says it works; a Chat's only until its reply streams. */
  session: boolean;
  /** The request can be answered again now. */
  rework: boolean;
  /** The request this block answers (the id of its user message). */
  requestId: string;
  /** It and the requests steered into it. */
  requestIds: string[];
  /** The message the user rates: the block's last reply. */
  answerId: string | null;
};

/** Seconds since `from`, ticking while `live`. */
function useElapsed(from: number, to: number | null, live: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [live]);
  return Math.max(0, (live || to === null ? now : to) - from);
}

function headerLabel(state: BlockState, elapsed: number, quota = false): string {
  const time = formatDuration(elapsed);
  switch (state) {
    case "working":
      if (quota) return `Waiting for quota · ${time}`;
      return elapsed < 1000 ? "Working" : `Working for ${time}`;
    case "waiting":
      return `Waiting for you · ${time}`;
    case "done":
      return `Worked for ${time}`;
    case "stopped":
      return `You stopped after ${time}`;
    case "failed":
      return `Failed after ${time}`;
  }
}

/** A phase's header: "Phase 1 · Measure — ✓ verified · 2h 31m", ticking only while it works. */
function phaseLabel(phase: PhaseView, elapsed: number): string {
  const state = phase.mark ? `${phase.mark} ${phase.word}` : phase.word;
  return phase.startedAtMs === null ? `${phase.title} — ${state}` : `${phase.title} — ${state} · ${formatDuration(elapsed)}`;
}

/** A live turn shows no header until it has worked this long (only "Thinking"). */
const HEADER_AFTER_MS = 2000;

/**
 * The row over a request's work, with a rule under it: "Working for 12s" while live, "Worked
 * for 3m 4s ›" once its work folds, "Waiting for quota · 12s" while its messages wait for a
 * model that can take them.
 */
const WorkHeader: FC<{
  meta: BlockMeta;
  /** An overnight phase's block: its header comes from the phase's record. */
  phase: PhaseView | null;
  open: boolean;
  foldable: boolean;
  /** Its messages wait for quota, and no worker of it runs meanwhile. */
  quota: boolean;
  /** Called with the header, before the fold opens or closes. */
  onToggle: (header: HTMLElement) => void;
}> = ({ meta, phase, open, foldable, quota, onToggle }) => {
  const elapsed = useElapsed(
    phase?.startedAtMs ?? meta.startedAtMs,
    phase ? phase.endedAtMs : meta.endedAtMs,
    phase ? !phase.settled : isLive(meta.state),
  );
  if (!phase && meta.state === "working" && !foldable && !quota && elapsed < HEADER_AFTER_MS) return null;
  const label = phase ? phaseLabel(phase, elapsed) : headerLabel(meta.state, elapsed, quota);
  const text = (
    <span
      className={cn(
        "text-sm",
        meta.state === "failed" && !phase ? "text-destructive" : "text-muted-foreground",
      )}
    >
      {label}
    </span>
  );
  if (!foldable) {
    return (
      <div
        data-slot="request-work-header"
        className="border-border flex h-control-sm items-center border-b"
      >
        {text}
      </div>
    );
  }
  return (
    <button
      type="button"
      data-slot="request-work-header"
      aria-expanded={open}
      onClick={(event) => onToggle(event.currentTarget)}
      className="group border-border flex h-control-sm items-center gap-1 border-b text-start"
    >
      {text}
      <ChevronRight
        aria-hidden
        className={cn(
          "text-muted-foreground size-icon-xs transition-[rotate] duration-150 ease-in-out motion-reduce:transition-none",
          open ? "rotate-90" : "opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100",
        )}
      />
    </button>
  );
};

function entryKey(entry: Entry): string {
  switch (entry.kind) {
    case "text":
      return `text:${entry.index}`;
    case "card":
      return `${entry.card.type}:${entry.card.id}`;
    case "steer":
      return `steer:${entry.position}`;
    case "orchestrator":
      return `orchestrator:${entry.position}`;
    case "compaction":
      return `compaction:${entry.compaction.id}`;
    case "row":
      return entry.row.type === "task" ? `task:${entry.row.taskId}` : `checks:${entry.row.phaseId}`;
  }
}

function CardEntry({ card }: { card: BlockCard }) {
  return (
    <div data-slot="request-card">
      <Suspense fallback={null}>
        <CardBody type={card.type} id={card.id} />
      </Suspense>
    </div>
  );
}

/** One reply of the block; text that still streams fades in word by word. */
const ReplyText: FC<{ index: number; streaming: boolean }> = ({ index, streaming }) => (
  <MessagePrimitive.PartByIndex
    index={index}
    components={{ Text: streaming ? StreamingMessageText : MessageText }}
  />
);

/** A reply, card or row in the block's work. */
const SequenceEntry: FC<{ entry: Entry; streaming: boolean }> = ({ entry, streaming }) => {
  switch (entry.kind) {
    case "text":
      return (
        <div
          data-slot="aui_assistant-message-content"
          className="text-foreground leading-relaxed wrap-break-word"
        >
          <ReplyText index={entry.index} streaming={streaming} />
        </div>
      );
    case "card":
      return <CardEntry card={entry.card} />;
    case "steer":
      return <SteerBubble text={entry.text} atMs={entry.atMs} />;
    case "orchestrator":
      return <OrchestratorSteps steps={entry.steps} />;
    case "compaction":
      return <CompactionRow compaction={entry.compaction} />;
    case "row":
      return entry.row.type === "task" ? (
        <TaskRow taskId={entry.row.taskId} />
      ) : (
        <PhaseChecksRow runId={entry.row.runId} phaseId={entry.row.phaseId} />
      );
  }
};

function compactionLabel({ automatic, state }: BlockCompaction): string {
  switch (state) {
    case "running":
      return automatic ? "Context automatically compacting" : "Compacting context";
    case "done":
      return automatic ? "Context automatically compacted" : "Context compacted";
    case "failed":
      return "Couldn’t compact context";
  }
}

/** The compaction line: "Compacting context" shimmers while it runs, then stays grey. */
const CompactionRow: FC<{ compaction: BlockCompaction }> = ({ compaction }) => (
  <div data-slot="compaction" data-state={compaction.state} className={STEP_ROW}>
    <TextShorterConcise aria-hidden className="size-icon-md shrink-0" />
    <span className={cn("min-w-0 truncate", compaction.state === "running" && "shimmer")}>
      {compactionLabel(compaction)}
      {compaction.error && ` · ${compaction.error}`}
    </span>
  </div>
);

/**
 * A compaction the user asked for between turns, after the answer it followed: while it runs
 * a turn of its own ("Working for 5s" over the shimmering row), then only its row, with no
 * header or actions.
 */
const CompactionBlock: FC<{ compaction: BlockCompaction }> = ({ compaction }) => {
  const live = compaction.state === "running";
  const elapsed = useElapsed(compaction.startedAtMs, compaction.endedAtMs, live);
  return (
    <div data-slot="compaction-block" className="mt-4 flex flex-col gap-2">
      {live && elapsed >= HEADER_AFTER_MS && (
        <div className="border-border flex h-control-sm items-center border-b">
          <span className="text-muted-foreground text-sm">{headerLabel("working", elapsed)}</span>
        </div>
      )}
      <CompactionRow compaction={compaction} />
    </div>
  );
};

/** A follow-up the user sent while the block worked: their bubble, inside the block. */
const SteerBubble: FC<{ text: string; atMs: number }> = ({ text, atMs }) => {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  return (
    <div
      data-slot="request-steer"
      className="group/steer flex max-w-7/10 min-w-0 flex-col items-end gap-y-1 self-end"
    >
      <div className="bg-muted text-foreground rounded-thread px-4 py-2 whitespace-pre-wrap wrap-break-word">
        <MentionText text={text} />
      </div>
      <div className="text-muted-foreground flex items-center gap-1 opacity-0 transition-opacity group-hover/steer:opacity-100 group-focus-within/steer:opacity-100">
        <span className="pe-1 text-xs tabular-nums">{formatSentAt(atMs, now)}</span>
        <TooltipIconButton tooltip={isCopied ? "Copied" : "Copy"} onClick={() => copyToClipboard(text)}>
          {isCopied ? <Check /> : <Copy />}
        </TooltipIconButton>
      </div>
    </div>
  );
};

/**
 * The last line of a working block: what happens right now ("Thinking", "Delegating…"), or
 * the worker it waits for ("Waiting for [Add tests]").
 */
const ActivityRow: FC<{ requestIds: string[] }> = ({ requestIds }) => {
  const { label, worker } = useBoard(
    useShallow((s): { label: string | null; worker: string | null } => {
      const board = s.board;
      if (!board) return { label: null, worker: null };
      const turn =
        board.runRequest !== null &&
        requestIds.includes(board.runRequest) &&
        (board.run === "running" || board.run === "starting");
      if (turn) {
        // Streaming text shows itself.
        const doing = board.doing || (board.streaming?.text ? null : "Thinking");
        return { label: doing, worker: null };
      }
      const working = Object.values(board.tasks)
        .filter((task) => task.requestId !== null && requestIds.includes(task.requestId) && isWorking(task))
        .toSorted((a, b) => a.number - b.number);
      const [first] = working;
      if (working.length === 1 && first) return { label: "Waiting for", worker: first.id };
      return {
        label: working.length > 1 ? `Waiting for ${working.length} workers` : null,
        worker: null,
      };
    }),
  );
  if (!label) return null;
  if (worker) {
    return (
      <div data-slot="request-activity" className="flex min-w-0 items-center gap-1.5 text-sm">
        <span className="shimmer shrink-0">{label}</span>
        <WorkerMention taskId={worker} />
      </div>
    );
  }
  return (
    // As wide as its words, so the sweep crosses them rather than the whole row.
    <div data-slot="request-activity" className="shimmer w-fit max-w-full truncate text-sm">
      {label}
    </div>
  );
};

/**
 * Where a turn is, for the thread's scrolling: its final answer streams (`final_answer`), it
 * shows work before that (`prework`), or it is over or shows nothing yet (`idle`).
 */
function turnPhase(
  meta: BlockMeta,
  live: boolean,
  answering: boolean,
): "idle" | "prework" | "final_answer" {
  if (!live) return "idle";
  const streaming = meta.texts.at(-1)?.position === Number.POSITIVE_INFINITY;
  const work = meta.rows.length > 0 || meta.orchestratorSteps.length > 0 || meta.cards.length > 0;
  if (answering || (!work && streaming)) return "final_answer";
  if (work || meta.texts.some((text) => text.position !== Number.POSITIVE_INFINITY)) return "prework";
  return "idle";
}

/**
 * One request's answer, shown as a turn. While the request works (and after it was stopped
 * or failed) its work shows in place, in order: replies, workers and cards, then what happens
 * right now. Once it is done, everything before the answer folds into "Worked for 3m 4s"; the
 * cards that matter (decisions, failures, what needs the user) and the user's follow-ups stay
 * in view.
 */
export const RequestBlock: FC = () => {
  const meta = useAuiState((s) => s.message.metadata.custom["block"]) as BlockMeta | undefined;
  const fold = useFold();
  const quotaWait = useViewConversation()?.quotaWait ?? null;
  const requestIds = meta?.requestIds;
  const phase = useBoard(
    useShallow((s) => (meta && s.board && isRunRequest(meta.requestId) ? phaseViewOf(s.board.overnight, meta.requestId) : null)),
  );
  const workersActive = useBoard((s) =>
    Object.values(s.board?.tasks ?? {}).some(
      (task) => task.requestId !== null && !!requestIds?.includes(task.requestId) && !isFinal(task),
    ),
  );
  if (!meta) return null;

  // A phase is live until it settles, whatever still waits on the user: that waits in the panel.
  const live = phase ? !phase.settled : isLive(meta.state);
  const last = meta.texts.length - 1;
  // The final answer is streaming: the workers it waited for are all over. The work folds now,
  // when the final answer starts.
  const answering =
    meta.state === "working" &&
    meta.rows.length > 0 &&
    !workersActive &&
    meta.texts[last]?.position === Number.POSITIVE_INFINITY;
  const done = phase ? phase.settled : meta.state === "done" || answering;
  // A settled phase folds to its outcome; its lead's replies go into the fold.
  const answer = done && last >= 0 && !phase ? last : null;
  const sequence = blockSequence(meta);
  const folded = sequence.filter((entry) =>
    entry.kind === "text"
      ? entry.index !== answer
      : entry.kind !== "card" || !entry.card.keep,
  );
  const kept = meta.cards.filter((card) => card.keep);
  const foldable = done && folded.length > 0;
  const outcome = done ? (phase?.outcome ?? null) : null;
  const shown = foldable && fold.state !== null;
  const turn = turnPhase(meta, live, answering);
  const header =
    phase !== null ||
    foldable ||
    meta.state === "stopped" ||
    meta.state === "failed" ||
    (live && (meta.session || meta.texts.length === 0 || meta.steers.length > 0));

  return (
    <MessagePrimitive.Root
      data-slot="aui_assistant-message-root"
      data-role="assistant"
      id={meta.answerId ? `message-${meta.answerId}` : undefined}
      tabIndex={-1}
      data-state={meta.state}
      data-turn-phase={turn}
      data-turn-live={live ? "true" : undefined}
      data-turn-steers={String(meta.steers.length)}
      className="group/answer relative flex flex-col gap-2 px-2"
    >
      {/* A run picks each phase lead's model itself: not a change the user made. */}
      {!phase && <ModelChanged model={meta.texts[last]?.model ?? null} picked={meta.picked} />}
      {header && (
        <WorkHeader
          meta={meta}
          phase={phase}
          open={fold.open}
          foldable={foldable}
          quota={quotaWait !== null && meta.state === "working" && !workersActive}
          onToggle={fold.toggle}
        />
      )}
      {done ? (
        <>
          {shown && (
            <div
              data-slot="request-fold"
              data-fold={fold.state}
              className={cn(
                "grid grid-rows-[1fr]",
                fold.state === "opening" && "animate-fold-open motion-reduce:animate-fold-fade-in",
                fold.state === "closing" && "animate-fold-close motion-reduce:animate-fold-fade-out",
              )}
            >
              {/* min-w-0: a long unbroken line (a branch in code) wraps instead of widening the fold. */}
              <div className={cn("flex min-h-0 min-w-0 flex-col gap-3", fold.state !== "open" && "overflow-hidden")}>
                {folded.map((entry) => (
                  <SequenceEntry key={entryKey(entry)} entry={entry} streaming={false} />
                ))}
              </div>
            </div>
          )}
          {/* The user's own follow-ups stay in view when the work folds. */}
          {!shown &&
            meta.steers.map((steer) => (
              <SteerBubble key={`steer:${steer.position}`} text={steer.text} atMs={steer.atMs} />
            ))}
          {kept.map((card) => (
            <CardEntry key={`${card.type}:${card.id}`} card={card} />
          ))}
          {outcome && (
            <p data-slot="phase-outcome" className="text-foreground text-sm leading-relaxed wrap-break-word">
              {outcome}
            </p>
          )}
          {answer !== null && (
            <div
              data-slot="aui_assistant-message-content"
              className="text-foreground leading-relaxed wrap-break-word"
            >
              <ReplyText index={answer} streaming={answering} />
            </div>
          )}
        </>
      ) : (
        <div data-slot="request-work" data-follow-content className="flex flex-col gap-3">
          {sequence.map((entry) => (
            <SequenceEntry
              key={entryKey(entry)}
              entry={entry}
              streaming={
                entry.kind === "text" &&
                meta.texts[entry.index]?.position === Number.POSITIVE_INFINITY
              }
            />
          ))}
          {(meta.state === "working" || (phase !== null && live)) && <ActivityRow requestIds={meta.requestIds} />}
        </div>
      )}
      {/* A run's work is merged from its card, never undone behind the run's back. */}
      {!live && meta.session && !isRunRequest(meta.requestId) && <TurnDiff requestId={meta.requestId} />}
      {!meta.session && <TurnMemories requestIds={meta.requestIds} />}
      <MessageError />
      {!live && last >= 0 && !phase && (
        <AnswerActions
          session={meta.session}
          rework={meta.rework}
          atMs={meta.endedAtMs}
          answerId={meta.answerId}
        />
      )}
      <ContinuedFrom answerId={meta.answerId} />
      {meta.compactions
        .filter((compaction) => !compaction.inTurn)
        .map((compaction) => (
          <CompactionBlock key={compaction.id} compaction={compaction} />
        ))}
    </MessagePrimitive.Root>
  );
};

/** How long the folded work takes to open and to close; keep in step with globals.css. */
const FOLD_OPEN_MS = 300;
const FOLD_CLOSE_MS = 150;

/**
 * The folded work of a turn: shut (`null`), opening, open or closing. It stays mounted while it
 * closes, and the header keeps its place on screen while the work opens or closes under it.
 */
function useFold() {
  const [state, setState] = useState<"opening" | "open" | "closing" | null>(null);
  useEffect(() => {
    if (state !== "opening" && state !== "closing") return;
    const timer = window.setTimeout(
      () => setState(state === "opening" ? "open" : null),
      state === "opening" ? FOLD_OPEN_MS : FOLD_CLOSE_MS,
    );
    return () => window.clearTimeout(timer);
  }, [state]);
  const open = state === "opening" || state === "open";
  const toggle = (header: HTMLElement) => {
    preserveAnchor(header);
    setState(open ? "closing" : "opening");
  };
  return { state, open, toggle };
}

/** The line shown when a turn ran on another model than the one picked (a fallback). */
const ModelChanged: FC<{ model: ModelChoice | null; picked: ModelChoice | null }> = ({
  model,
  picked,
}) => {
  const groups = useModelGroups();
  if (!model || !picked || sameModel(groups, picked, model)) return null;
  return (
    <p className="text-muted-foreground text-sm">
      Model changed from {modelName(groups, picked)} to {modelName(groups, model)}.
    </p>
  );
};

/**
 * Under the answer: copy it, rate it and, in a Chat, ask for another answer and move between
 * answers (a session's answers have neither). The latest answer always shows them, an older one
 * on hover or focus; when it came shows on hover.
 */
const AnswerActions: FC<{
  session: boolean;
  rework: boolean;
  atMs: number | null;
  answerId: string | null;
}> = ({ session, rework, atMs, answerId }) => {
  const conversation = useViewConversation();
  const answer = useAuiState((s) => {
    const parts = s.message.parts;
    const part = parts[parts.length - 1];
    return part?.type === "text" ? part.text : "";
  });
  const rated = useAuiState((s) => {
    const type = s.message.metadata.submittedFeedback?.type;
    return type === "positive" ? "good" : type === "negative" ? "bad" : null;
  });
  const latest = useAuiState((s) => s.message.isLast);
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  return (
    <ActionBarPrimitive.Root
      autohide="never"
      className={cn(
        "text-muted-foreground -ms-1 flex min-h-7.5 items-center gap-1",
        !latest && "opacity-0 group-hover/answer:opacity-100 group-focus-within/answer:opacity-100",
      )}
    >
      <TooltipIconButton tooltip={isCopied ? "Copied" : "Copy"} onClick={() => copyToClipboard(answer)}>
        {isCopied ? (
          <Check className="animate-in zoom-in-50 fade-in duration-200 ease-out" />
        ) : (
          <Copy className="animate-in zoom-in-75 fade-in duration-150" />
        )}
      </TooltipIconButton>
      <RateMenu rated={rated}>
        <ActionBarPrimitive.FeedbackPositive asChild>
          <RateItem rating="good" />
        </ActionBarPrimitive.FeedbackPositive>
        <ActionBarPrimitive.FeedbackNegative asChild>
          <RateItem rating="bad" />
        </ActionBarPrimitive.FeedbackNegative>
      </RateMenu>
      <ReadAloud text={answer} />
      {!session && rework && (
        <ActionBarPrimitive.Reload asChild>
          <TooltipIconButton tooltip="Try again">
            <Regenerate />
          </TooltipIconButton>
        </ActionBarPrimitive.Reload>
      )}
      {!session && <BranchPicker />}
      {conversation && answerId && (
        <ForkMenu conversationId={conversation.id} kind={conversation.kind} messageId={answerId} />
      )}
      {atMs !== null && (
        <span className="ps-1 text-xs tabular-nums opacity-0 group-hover/answer:opacity-100 group-focus-within/answer:opacity-100">
          {formatSentAt(atMs, now)}
        </span>
      )}
    </ActionBarPrimitive.Root>
  );
};

/** "Read aloud": the answer in the system's voice; "Stop" while it reads. */
const ReadAloud: FC<{ text: string }> = ({ text }) => {
  const id = useAuiState((s) => s.message.id);
  const reading = useReading(id);
  if (!text) return null;
  return reading ? (
    <TooltipIconButton tooltip="Stop" onClick={stopReading}>
      <Stop />
    </TooltipIconButton>
  ) : (
    <TooltipIconButton tooltip="Read aloud" onClick={() => readAloud(id, text)}>
      <SoundOnReadOutLoudSpeaker />
    </TooltipIconButton>
  );
};

/** "⑂ Continued from chat" under the answer a fork was made from: back to where it came from. */
const ContinuedFrom: FC<{ answerId: string | null }> = ({ answerId }) => {
  const origin = useViewConversation()?.forkedFrom ?? null;
  const source = useApp((s) => (origin ? s.conversations[origin.conversationId] : undefined));
  if (!origin || !answerId || origin.messageId !== answerId) return null;
  return (
    <div className="text-muted-foreground flex items-center gap-3 py-2 text-sm">
      <span className="bg-border h-px flex-1" />
      {source ? (
        <button
          type="button"
          onClick={() => openConversation(source.id)}
          className="text-link hover:text-link/80 flex items-center gap-1.5"
        >
          <Branch className="size-icon-sm" />
          Continued from chat
        </button>
      ) : (
        <span className="flex items-center gap-1.5">
          <Branch className="size-icon-sm" />
          Continued from chat
        </span>
      )}
      <span className="bg-border h-px flex-1" />
    </div>
  );
};
