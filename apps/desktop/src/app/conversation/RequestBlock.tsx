import {
  ActionBarPrimitive,
  MessagePrimitive,
  type TextMessagePartProps,
  useAui,
  useAuiState,
} from "@assistant-ui/react";
import {
  Branch,
  Check,
  ChevronRight,
  Copy,
  Info,
  Regenerate,
  SoundOnReadOutLoudSpeaker,
  Stop,
  TextShorterConcise,
} from "@openai/apps-sdk-ui/components/Icon";
import { type ComponentProps, type ComponentType, type FC, Fragment, lazy, Suspense, useCallback, useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { ThinkingLine } from "@/app/conversation/ThinkingLine";
import { WorkFold } from "@/app/conversation/WorkFold";
import { ForkMenu } from "@/app/conversation/ForkMenu";
import { InlineImageText } from "@/app/conversation/InlineImage";
import { UserMarkdownText } from "@/app/conversation/Mentions";
import { ThreadStep } from "@/app/conversation/activity/Notice";
import { ActivityGroup } from "@/app/conversation/activity/ActivityGroup";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { type Activity, type LeadStep, thinkingLabel, turnActivity } from "@/app/conversation/activity/group";
import { deniedCall } from "@/app/conversation/activity/words";
import { waitingWords, went } from "@/app/conversation/activity/peerWords";
import { describeLeadStep, LeadStepRow } from "@/app/conversation/activity/LeadStep";
import { describePeerStep, peerStepRow, usePeerLookup } from "@/app/conversation/activity/PeerRow";
import { PeerMessageBlock } from "@/app/conversation/PeerMessageBlock";
import { CHEVRON, ROW, ROW_TOGGLE } from "@/components/assistant-ui/elements/activity-row";
import {
  type BlockCard,
  type BlockCompaction,
  type BlockOrchestratorStep,
  type BlockRow,
  type BlockState,
  type PeerOf,
  answerIndex,
  answeringIndex,
  blockFinished,
  endsWithPlan,
  blockSequence,
  foldsAway,
  isFinal,
  isLive,
  isRunRequest,
  type SequenceEntry as Entry,
  updateIndices,
  turnTime,
  waitedTime,
  waitsOnCard,
} from "@/app/conversation/blocks";
import { type PhaseView, phaseViewOf, splitReport } from "@/app/conversation/phaseView";
import { TeamSentence } from "@/app/conversation/activity/TeamSentence";
import type { StatusHead } from "@/app/conversation/liveStatus";
import { HeldNote } from "@/app/conversation/HeldNote";
import { AnsweredStatus, StatusLine, type ThreadStatusProps, useThreadStatusHead } from "@/app/conversation/ThreadStatus";
import { TurnDiff } from "@/app/conversation/TurnDiff";
import { AnswerFiles } from "@/app/conversation/AnswerFiles";
import { TurnMemories } from "@/app/conversation/TurnMemories";
import { describeTurnError, interruptedByQuit } from "@/app/conversation/turnError";
import { useAction } from "@/app/conversation/useAction";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ErrorState } from "@/components/assistant-ui/elements/error-state";
import {
  BranchPicker,
  MarkdownBlock,
  CommentaryMessageText,
  MessageText,
  StreamingMessageText,
} from "@/components/assistant-ui/thread";
import { RateItem, RateMenu } from "@/components/assistant-ui/rate-menu";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import type { AttachmentRef, ModelChoice, ThinkingSegment, WorkSpan } from "@/ipc/generated";
import { formatDuration, formatSentAt } from "@/lib/format";
import { VENDOR_LABELS } from "@/lib/routing";
import { modelName, sameModel, useModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { openConversation, resume, send } from "@/state/actions";
import { readAloud, stopReading, useReading } from "@/state/readAloud";
import { RETRYING, useBoard } from "@/state/board";
import { useRevealing } from "@/state/reveal";
import { useApp } from "@/state/store";

/**
 * The card views load on first use, and once loaded render at once: a card in folded work must
 * be whole the frame its work opens, or the row pops in after the fold has measured its height.
 */
let loadedCardBody: ComponentType<ComponentProps<typeof LazyCardBody>> | null = null;
const loadCardBody = () =>
  import("@/app/conversation/cards/CardBody").then((module) => {
    loadedCardBody = module.default;
    return module;
  });
const LazyCardBody = lazy(loadCardBody);

/** What an assistant block carries in its message metadata (`custom.block`). */
export type BlockMeta = {
  /** Per text part, in order: the reply's position (infinite while it streams) and model. */
  texts: {
    position: number;
    model: ModelChoice | null;
    update: boolean;
    anchors?: readonly string[];
    /** Its CLI said it is the turn's final answer: when it started. */
    answerAtMs?: number;
    /** It streams as a line between the turn's steps. */
    commentary?: boolean;
  }[];
  cards: BlockCard[];
  /** The workers' rows, in the order they started. */
  rows: BlockRow[];
  /** The orchestrator's steps, in order. */
  orchestratorSteps: BlockOrchestratorStep[];
  thinking?: ThinkingSegment[];
  /** Messages the user steered into the turn, shown as bubbles in the work. */
  steers: { position: number; text: string; atMs: number; attachments: AttachmentRef[]; requestId?: string; peer?: PeerOf }[];
  /** Compactions of a Chat's context in the turn, or after it. */
  compactions: BlockCompaction[];
  state: BlockState;
  startedAtMs: number;
  endedAtMs: number | null;
  /** When its requests worked, for "Worked for …". */
  worked: WorkSpan[];
  /** When its requests waited for the answers to their question cards, for "waited for you …". */
  waited: WorkSpan[];
  /** It waits only for quota, not for the user. */
  quotaWait: boolean;
  /** It waits for the answer to its own question card, so it still reads as working. */
  cardWait: boolean;
  /** The model the user picked, to flag a fallback. */
  picked: ModelChoice | null;
  /** A session's block always says it works; a Chat's only until its reply streams. */
  session: boolean;
  /** The request can be answered again now. */
  rework: boolean;
  /** The request this block answers (the id of its user message). */
  requestId: string;
  /** The sessions whose replies it rests waiting for. */
  waitingFor: string[];
  /** It and the requests steered into it. */
  requestIds: string[];
  /** The message the user rates: the block's answer, not an update line under it. */
  answerId: string | null;
  /** The message a fork starts from: the block's last reply, an update line included. */
  forkId: string | null;
};

/** Now, ticking each second while `live`. */
function useTicking(live: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [live]);
  return now;
}

/** Seconds since `from`, ticking while `live`. */
function useElapsed(from: number, to: number | null, live: boolean): number {
  const now = useTicking(live);
  return Math.max(0, (live || to === null ? now : to) - from);
}

/**
 * `waited`: how long it waited for the answers to its question cards, shown apart from the work
 * (BUILD-PLAN A9); `onCard`: a card waits for the user now.
 */
function headerLabel(state: BlockState, elapsed: number, quota = false, waitingFor: readonly string[] = [], waited = 0, onCard = false, interrupted = false): string {
  const time = formatDuration(elapsed);
  switch (state) {
    case "working": {
      if (quota) return `Waiting for quota · ${time}`;
      const work = elapsed < 1000 ? "Working" : `Working for ${time}`;
      return onCard ? `${work} · waiting for you ${formatDuration(waited)}` : work;
    }
    case "waiting":
      if (quota) return `Waiting for quota · ${time}`;
      return waitingFor.length > 0 ? `${waitingWords(waitingFor)} · ${time}` : `Waiting for you · ${time}`;
    case "done":
      return waited >= 1000 ? `Worked ${time} · waited for you ${formatDuration(waited)}` : `Worked for ${time}`;
    case "stopped":
      return `You stopped after ${time}`;
    // Said calmly: the error row under the turn tells what happened.
    case "failed":
      return interrupted ? `Interrupted after ${time}` : `Stopped after ${time}`;
  }
}

/**
 * An overnight run's header: "Overnight · Windows support — Phase 2 of 3 · Fix · 1h 4m" while it
 * works, "— ◐ 1 of 3 phases done · 6h 12m" once over; ticking only while it works.
 */
function phaseLabel(phase: PhaseView, elapsed: number): string {
  const state = phase.mark ? `${phase.mark} ${phase.word}` : phase.word;
  return phase.startedAtMs === null ? `${phase.title} — ${state}` : `${phase.title} — ${state} · ${formatDuration(elapsed)}`;
}

/**
 * The row over a request's work, with a rule under it: "Working for 12s" while live, "Worked
 * for 3m 4s ›" once its work folds, "Waiting for quota · 12s" while its messages wait for a
 * model that can take them.
 */
const WorkHeader: FC<{
  meta: BlockMeta;
  /** An overnight run's block: its header comes from the run and its plan. */
  phase: PhaseView | null;
  open: boolean;
  foldable: boolean;
  /** Its messages wait for quota, and no worker of it runs meanwhile. */
  quota: boolean;
  /** The sessions whose replies it waits for, by title. */
  waitingFor: readonly string[];
  /** Its final answer streams or stands while the request still runs: its work is over. */
  answering: boolean;
  /** When its final answer started, where its time stops. */
  answerAtMs: number | undefined;
  onToggle: () => void;
}> = ({ meta, phase, open, foldable, quota, waitingFor, answering, answerAtMs, onToggle }) => {
  const phaseElapsed = useElapsed(phase?.startedAtMs ?? 0, phase?.endedAtMs ?? null, !!phase && !phase.settled);
  const now = useTicking(!phase && isLive(meta.state));
  const elapsed = phase ? phaseElapsed : turnTime(meta, now, answerAtMs);
  const waited = waitedTime(meta, now, answerAtMs);
  // Waiting on its own question card, the turn still reads as working, with the wait beside it.
  const onCard = !answering && waitsOnCard(meta);
  const state = answering ? "done" : onCard ? "working" : meta.state;
  const interrupted = useAuiState((s) => {
    const status = s.message.status;
    return status?.type === "incomplete" && status.reason === "error" && interruptedByQuit(String(status.error ?? ""));
  });
  const label = phase ? phaseLabel(phase, elapsed) : headerLabel(state, elapsed, quota || meta.quotaWait, waitingFor, waited, onCard, interrupted);
  const text = (
    <span
      data-slot="request-work-label"
      className={cn(
        "text-foreground/50 text-sm leading-(--spacing-activity-row) tabular-nums",
        foldable && "group-hover:text-foreground group-focus-visible:text-foreground",
      )}
    >
      {label}
    </span>
  );
  // The label, 8px, then a hairline rule across the column.
  const rule = "border-work-rule flex items-center border-b pb-2";
  if (!foldable) {
    return (
      <div data-slot="request-work-header" className={rule}>
        {text}
      </div>
    );
  }
  return (
    <button
      type="button"
      data-slot="request-work-header"
      aria-expanded={open}
      onClick={onToggle}
      className={cn(rule, "group gap-1 text-start")}
    >
      {text}
      <ChevronRight
        aria-hidden
        className={cn(
          "text-foreground/50 size-chevron group-hover:text-foreground group-focus-visible:text-foreground shrink-0 transition-[rotate,color] duration-150 ease-standard motion-reduce:transition-none",
          open && "rotate-90",
        )}
      />
    </button>
  );
};

function entryKey(entry: Entry): string {
  switch (entry.kind) {
    case "thinking":
      return `thinking:${entry.segment.itemId}`;
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
      return `task:${entry.row.taskId}:${entry.position}`;
  }
}

function CardEntry({ card }: { card: BlockCard }) {
  // Chosen once: a card shown through the lazy view keeps it, so it never remounts.
  const [CardBody] = useState(() => loadedCardBody ?? LazyCardBody);
  return (
    <div data-slot="request-card">
      <Suspense fallback={null}>
        <CardBody type={card.type} id={card.id} />
      </Suspense>
    </div>
  );
}

/**
 * One reply of the block; text that still streams fades in word by word. A line between the
 * turn's steps (`commentary`) streams as prose like the answer, but shows as it arrives: only the
 * final answer is paced.
 */
const ReplyText: FC<{ index: number; streaming: boolean; report?: boolean; commentary?: boolean }> = ({ index, streaming, report = false, commentary = false }) => (
  <MessagePrimitive.PartByIndex
    index={index}
    components={{ Text: report ? ReportText : streaming ? (commentary ? CommentaryMessageText : StreamingMessageText) : MessageText }}
  />
);

/**
 * A session's answer or a run's morning report: what it came to in view, its `### Details`
 * (the checks run, review findings, what wasn't tested) folded under "Details".
 */
const ReportText: FC<TextMessagePartProps> = (props) => {
  const report = splitReport(props.text);
  if (!report) return <MessageText {...props} />;
  return (
    <div className="flex flex-col gap-3">
      <MarkdownBlock text={report.head} />
      <details data-slot="report-details" className="group/details">
        {/* Folds like a work row: its words, then the same quiet chevron, never the browser's marker. */}
        <summary className={cn(ROW, ROW_TOGGLE, "list-none [&::-webkit-details-marker]:hidden")}>
          Details
          <ChevronRight aria-hidden className={cn(CHEVRON, "group-open/details:rotate-90")} />
        </summary>
        <div className="pt-2">
          <MarkdownBlock text={report.details} />
        </div>
      </details>
    </div>
  );
};

/** The files under the answer at `index`, which it may link corrected copies of. */
const AnswerFilesOf: FC<{ index: number; requestIds: readonly string[] }> = ({ index, requestIds }) => {
  const answer = useAuiState((s) => {
    const part = s.message.parts[index];
    return part?.type === "text" ? part.text : "";
  });
  return <AnswerFiles requestIds={requestIds} answer={answer} />;
};

/** A reply, card or row in the block's work. */
const SequenceEntry: FC<{ entry: Entry; streaming: boolean; commentary?: boolean }> = ({ entry, streaming, commentary = false }) => {
  switch (entry.kind) {
    case "thinking":
      // Thinking is never a row: the live thought feeds the live line.
      return null;
    case "text":
      return (
        <div
          data-slot="aui_assistant-message-content"
          data-commentary={commentary || undefined}
          // The replies Brigadier sent other sessions with this answer land a jump here.
          data-peer-message={entry.anchors?.join(" ")}
          className="text-foreground leading-relaxed wrap-break-word"
        >
          <ReplyText index={entry.index} streaming={streaming} commentary={commentary} />
        </div>
      );
    case "card":
      return <CardEntry card={entry.card} />;
    case "steer":
      // Another session's message steered in, or a reply into its request's turn.
      if (entry.peer) return <PeerMessageBlock peer={entry.peer} text={entry.text} atMs={entry.atMs} />;
      return <SteerBubble text={entry.text} atMs={entry.atMs} attachments={entry.attachments} requestId={entry.requestId} />;
    case "orchestrator":
      return <>{entry.steps.map((step) => <ThreadStep key={step.position} step={step} />)}</>;
    case "compaction":
      return <CompactionRow compaction={entry.compaction} />;
    case "row":
      return <TeamSentence row={entry.row} />;
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
  <div data-slot="compaction" data-state={compaction.state} className={ROW}>
    <TextShorterConcise aria-hidden className="size-4 shrink-0" />
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
      {live && (
        <div className="border-border flex items-center border-b pb-2">
          <span className="text-muted-foreground text-sm">{headerLabel("working", elapsed)}</span>
        </div>
      )}
      <CompactionRow compaction={compaction} />
    </div>
  );
};

/** A follow-up the user sent while the block worked: their bubble, inside the block. */
const SteerBubble: FC<{ text: string; atMs: number; attachments: readonly AttachmentRef[]; requestId?: string | undefined }> = ({
  text,
  atMs,
  attachments,
  requestId,
}) => {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  // Sent while the thread is inside a command: it reads it once that ends.
  const held = useBoard((s) => (requestId ? s.board?.requests[requestId]?.heldBy : undefined));
  return (
    <div
      data-slot="request-steer"
      className="group/steer flex max-w-7/10 min-w-0 flex-col items-end gap-y-1 self-end"
    >
      <div className="bg-muted text-foreground rounded-bubble max-w-full min-w-0 px-4 py-2.5 wrap-anywhere [&_.aui-md-inline-code]:bg-foreground/10">
        <InlineImageText text={text} attachments={attachments} Text={UserMarkdownText} />
      </div>
      {held && <HeldNote held={held} />}
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
 * A turn's activity in order (THREAD-UX-PLAN.md §3.1): its work groups and everything else as it
 * is. The live and the folded turn render the same items, so nothing moves when the turn ends.
 * The live turn's newest work has one live line: what waits on the user, else what the lead does
 * ("Thinking", "Waiting for a worker"), else the live thought's heading. When the work ends on a
 * group of steps, that group's row says it, unless its step runs and says itself; otherwise the
 * line goes under the work.
 */
const ActivityItems: FC<{
  activity: readonly Activity<LeadStep, Entry>[];
  /** The live turn's newest work: what its live line reads from; `null` for any other. */
  live: ThreadStatusProps | null;
  /** The live thought, if the lead thinks right now. */
  thought: string | null;
  streaming: (entry: Entry) => boolean;
  /** A streaming reply that is a line between the steps, not an answer. */
  commentary: (entry: Entry) => boolean;
  /** The turn's requests, whose denied approvals its failed calls may be. */
  requestIds: readonly string[];
  /** A follow-up was steered in after this work: its groups sum up at once. */
  summary: boolean;
}> = ({ activity, live, thought, streaming, commentary, requestIds, summary }) => {
  const head = useThreadStatusHead(live);
  // Under a message just steered in, with what ran before it summed up, it thinks: even while that still runs.
  const line: (StatusHead & { thought?: boolean }) | null = !live
    ? null
    : (head ?? (thought === null ? (activity.length === 0 ? { text: "Thinking", tone: "busy" } : null) : { text: thinkingLabel(thought) || "Thinking", tone: "busy", thought: true }));
  // A message to another session says how its exchange stands itself: the line stays under it.
  const newest = activity.at(-1);
  const lastStep = newest?.type === "group" ? newest.items.findLast((item) => item.type === "step") : undefined;
  const inGroup = lastStep?.type === "step" && lastStep.step.kind.type !== "peerSent" && lastStep.step.kind.type !== "messageRefused";
  // A message the running lead just sent, with nothing after it yet: its row still reads as
  // sending and no line goes under it, so the wait that usually follows takes the row over and
  // the turn never shrinks under the user's message.
  const leadRunning = useBoard((s) => {
    const board = s.board;
    return !!live && !!board?.runRequest && live.requestIds.includes(board.runRequest) && (board.run === "running" || board.run === "starting");
  });
  const tasks = useBoard((s) => s.board?.tasks);
  const approvals = useBoard((s) => s.board?.approvals);
  const revealing = useRevealing(useViewConversation()?.id ?? null);
  const peers = usePeerLookup(
    activity.flatMap((item) => (item.type === "group" ? item.items.flatMap((step) => (step.type === "step" ? [step.step] : [])) : [])),
  );
  const sentStep = newest?.type === "group" && lastStep !== undefined && newest.items.at(-1) === lastStep
    && lastStep.type === "step" && lastStep.step.kind.type === "peerSent" ? lastStep.step : null;
  const going = leadRunning && thought === null && !live?.thinkingLive && sentStep?.kind.type === "peerSent"
    && sentStep.kind.waits.length === 0 && sentStep.kind.messageIds.length > 0
    && sentStep.kind.messageIds.every((id) => { const record = peers.records[id]; return !!record && went(record.state); })
    ? sentStep : null;
  // What waits on the user, or a retry stalling the turn, still shows.
  const shownLine = going && line?.tone !== "needsYou" && !RETRYING.has(line?.text ?? null) ? null : line;
  // A call the user's Deny stopped reads as declined, not failed.
  const shown = (step: LeadStep): LeadStep =>
    approvals && step.kind.type === "tool" && deniedCall(step.kind, step.atMs, requestIds, Object.values(approvals))
      ? { ...step, kind: { ...step.kind, status: "declined" } }
      : step;
  return (
    <>
      {activity.map((item, index) =>
        item.type === "group" ? (
          <ActivityGroup
            key={`group:${item.key}`}
            items={item.items}
            live={!!live && index === activity.length - 1}
            liveLabel={inGroup && index === activity.length - 1 ? line : null}
            summary={summary}
            describe={(step) => describePeerStep(step, peers, step === going) ?? describeLeadStep(shown(step), tasks)}
            renderStep={(step, key) => <Fragment key={key}>{peerStepRow(step, step === going) ?? <LeadStepRow step={shown(step)} />}</Fragment>}
            reveal={revealing ? (step) => step.kind.type === "peerSent" && step.kind.messageIds.includes(revealing) : undefined}
          />
        ) : (
          <SequenceEntry key={entryKey(item.entry)} entry={item.entry} streaming={streaming(item.entry)} commentary={commentary(item.entry)} />
        ),
      )}
      {!inGroup && shownLine && (shownLine.thought && thought !== null ? <ThinkingLine text={thought} /> : <StatusLine head={shownLine} />)}
    </>
  );
};

/** What a session's Try again sends, as the user's own message. */
const TRY_AGAIN = "Try again.";

/**
 * The block's error: one plain line, the error's own words under Details, and Try again where it
 * can be: a Chat answers again; a session's last turn gets "Try again." sent as the user's
 * message, the way the composer sends it. An overnight run's turn has none: the run goes on.
 */
const BlockError: FC<{ meta: BlockMeta; live: boolean }> = ({ meta, live }) => {
  const error = useAuiState((s) => {
    const status = s.message.status;
    return status?.type === "incomplete" && status.reason === "error"
      ? String(status.error ?? "The reply failed.")
      : null;
  });
  const latest = useAuiState((s) => s.message.isLast);
  const conversation = useViewConversation();
  const aui = useAui();
  const action = useAction();
  if (error === null) return null;
  const model = meta.texts.at(-1)?.model ?? meta.picked;
  const { summary, limit } = describeTurnError(error, model ? VENDOR_LABELS[model.provider] : undefined);
  // Brigadier quit while it ran: Continue picks the request up (UI audit H1).
  const interrupted = interruptedByQuit(error);
  const again = (() => {
    if (interrupted) {
      if (live || !latest || !conversation || conversation.lifecycle === "archived" || action.busy) return undefined;
      const id = conversation.id;
      return () => action.run(() => resume(id));
    }
    if (live || isRunRequest(meta.requestId)) return undefined;
    if (!meta.session) return meta.rework ? () => aui.message().reload() : undefined;
    if (!latest || !conversation || conversation.lifecycle === "archived" || action.busy) return undefined;
    const to = conversation.id;
    return () => action.run(() => send({ text: TRY_AGAIN, attachments: [], mentions: [] }, undefined, { to }));
  })();
  // A usage limit is no error of the turn: a divider says it, with Try again for later.
  if (limit) {
    return (
      <div data-slot="usage-limit" role="status" className="text-muted-foreground flex items-center gap-3 py-1 text-sm">
        <span aria-hidden className="bg-border h-px flex-1" />
        <Info aria-hidden className="size-icon-sm shrink-0" />
        <span className="min-w-0 text-center">{summary}</span>
        {again && (
          <button type="button" onClick={again} className="text-link hover:text-link/80 shrink-0">
            Try again
          </button>
        )}
        <span aria-hidden className="bg-border h-px flex-1" />
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-1">
      <ErrorState summary={summary} details={interrupted ? null : error} onRetry={again} retryLabel={interrupted ? "Continue" : undefined} />
      {action.error && (
        <p role="alert" className="text-muted-foreground px-3 text-xs">
          Couldn’t send it: {action.error}
        </p>
      )}
    </div>
  );
};

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
  // A jump to a message this turn sent opens its work.
  const revealing = useRevealing(useViewConversation()?.id ?? null);
  const holds = !!revealing && !!meta?.orchestratorSteps.some((step) => step.kind.type === "peerSent" && step.kind.messageIds.includes(revealing));
  const { show } = fold;
  useEffect(() => {
    if (holds) show();
  }, [holds, show]);
  const quotaWait = useViewConversation()?.quotaWait ?? null;
  const requestIds = meta?.requestIds;
  const phase = useBoard(
    useShallow((s) => (meta && s.board && isRunRequest(meta.requestId) ? phaseViewOf(s.board.overnight, s.board.plans, meta.requestId) : null)),
  );
  const workersActive = useBoard((s) =>
    Object.values(s.board?.tasks ?? {}).some(
      (task) => task.requestId !== null && !!requestIds?.includes(task.requestId) && !isFinal(task),
    )
    // A long run whose result the request is owed works like a worker: the thread's word on it
    // is a line in the work, and its answer comes with the result.
    || Object.values(s.board?.backgroundRuns ?? {}).some(
      (run) => run.state.type === "running" && run.requestId !== null && !!requestIds?.includes(run.requestId),
    ),
  );
  // A run's morning report folds its details.
  const report = useBoard((s) => {
    const id = meta?.answerId;
    return !!id && Object.values(s.board?.overnight ?? {}).some((run) => run.reportMessageId === id);
  });
  // The sessions it waits for, by title (one deleted meanwhile is no longer waited for).
  const waitingFor = useApp(useShallow((s) => (meta?.waitingFor ?? []).flatMap((id) => s.conversations[id]?.title ?? [])));
  // Its cards load before its work can open, so they are there the moment it does.
  const hasCards = (meta?.cards.length ?? 0) > 0;
  useEffect(() => {
    if (hasCards) void loadCardBody();
  }, [hasCards]);
  if (!meta) return null;

  // A run's block is live until the run is over, whatever still waits on the user: that waits in the panel.
  const live = phase ? !phase.settled : isLive(meta.state);
  const last = meta.texts.length - 1;
  const sequence = blockSequence(meta);
  const finished = blockFinished(meta.state, phase);
  // Its work folds the moment its CLI starts the final answer, though the request may still wait
  // (for another session's reply, say); with a CLI that doesn't say, once the request is done.
  // Until then a reply streams in place.
  const answering = !finished && !phase && live && !workersActive ? answeringIndex(meta.texts, sequence) : null;
  const done = finished || answering !== null;
  // A run over folds to its outcome; the thread's replies during it go into the fold.
  const answer = answering ?? (endsWithPlan(sequence) ? null : answerIndex(meta.texts, finished && !phase));
  const answerAtMs = answer === null ? undefined : meta.texts[answer]?.answerAtMs;
  const updates = updateIndices(meta.texts, answer);
  const activity = turnActivity(sequence);
  // The answer, kept cards and access notices stay outside the fold.
  const folded = activity.filter((item) => item.type === "group" || foldsAway(item.entry, answer));
  const kept = sequence.filter((entry) => entry.kind !== "text" && !foldsAway(entry, answer));
  const parts = workParts(done ? folded : activity, live && !done);
  const foldable = done && parts.some((part) => part.type === "work" && part.items.length > 0);
  const outcome = done ? (phase?.outcome ?? null) : null;
  const header =
    phase !== null ||
    foldable ||
    meta.state === "stopped" ||
    meta.state === "failed" ||
    // A Chat's says it works over its steps, and with none until its reply streams, once something came.
    (live && (meta.session || meta.steers.length > 0 || activity.some((item) => item.type === "group" && item.items.some((each) => each.type === "step")) || (meta.texts.length === 0 && sequence.length > 0)));
  const streams = (index: number) => meta.texts[index]?.position === Number.POSITIVE_INFINITY;
  const streaming = (entry: Entry) => !done && entry.kind === "text" && streams(entry.index);
  const commentary = (entry: Entry) => entry.kind === "text" && !!meta.texts[entry.index]?.commentary;
  const liveThought = sequence.findLast((entry) => entry.kind === "thinking" && entry.live);
  const status: ThreadStatusProps = {
    requestIds: meta.requestIds,
    // A live run works on, whatever the thread's last turn came to.
    state: isLive(meta.state) ? meta.state : "working",
    thinkingLive: !!liveThought,
    compacting: meta.compactions.some((compaction) => compaction.inTurn && compaction.state === "running"),
    quotaWait,
    waitingFor,
  };

  return (
    <MessagePrimitive.Root
      data-slot="aui_assistant-message-root"
      data-role="assistant"
      id={meta.answerId ? `message-${meta.answerId}` : undefined}
      tabIndex={-1}
      data-state={meta.state}
      className="group/answer gap-answer-actions-gap relative flex flex-col px-2"
    >
      {/* A run picks its models itself: not a change the user made. */}
      {!phase && <ModelChanged model={meta.texts[last]?.model ?? null} picked={meta.picked} />}
      {/* A step still running when the user stopped the turn reads "Stopped …". */}
      <StoppedBy.Provider value={meta.state === "stopped" ? { atMs: meta.endedAtMs } : null}>
      <div data-slot="request-body" className="flex min-w-0 flex-col">
        {header && (
          <WorkHeader
            meta={meta}
            phase={phase}
            open={fold.open}
            foldable={foldable}
            quota={quotaWait !== null && meta.state === "working" && !workersActive}
            waitingFor={waitingFor}
            answering={answering !== null}
            answerAtMs={answerAtMs}
            onToggle={fold.toggle}
          />
        )}
        {/* The work, cut at the user's follow-ups: each run of it folds on its own, and the
            bubbles between them stay mounted whether it is open or folded, so nothing below
            them pops in or out when it settles (THREAD-PARITY-PLAN.md §4.2). */}
        {parts.map((part, index) =>
          part.type === "steer" ? (
            <div key={entryKey(part.entry)} className={cn("flex min-w-0 flex-col", (header || index > 0) && "pt-activity")}>
              <SequenceEntry entry={part.entry} streaming={false} />
            </div>
          ) : (
            <WorkFold key={`work:${index}`} open={!done || fold.open}>
              <div
                data-slot="request-work"
                className={cn("gap-activity flex min-w-0 flex-col", (header || index > 0) && "pt-activity")}
              >
                <ActivityItems
                  activity={part.items}
                  requestIds={meta.requestIds}
                  live={live && !done && index === parts.length - 1 ? status : null}
                  thought={liveThought?.kind === "thinking" ? liveThought.segment.text : null}
                  streaming={streaming}
                  commentary={commentary}
                  // A follow-up steered in after it: the work before it is over, whatever still runs.
                  summary={parts.slice(index + 1).some((next) => next.type === "steer")}
                />
              </div>
            </WorkFold>
          ),
        )}
        {done && (
          <>
            {kept.map((entry) => (
              <div key={entryKey(entry)} className="pt-activity flex min-w-0 flex-col">
                <SequenceEntry entry={entry} streaming={false} />
              </div>
            ))}
            {outcome && (
              <p data-slot="phase-outcome" className="text-foreground pt-activity text-sm leading-relaxed wrap-break-word">
                {outcome}
              </p>
            )}
            {answer !== null && (
              <div
                data-slot="aui_assistant-message-content"
                // The replies Brigadier sent other sessions with this answer land a jump here.
                data-peer-message={meta.texts[answer]?.anchors?.join(" ")}
                className={cn("text-foreground leading-relaxed wrap-break-word", (header || parts.length > 0) && "pt-activity")}
              >
                <ReplyText index={answer} streaming={streams(answer)} report={!streams(answer) && (report || meta.session)} />
              </div>
            )}
            {/* Answered, it may still wait for another session's reply: that line stays under the answer. */}
            {answering !== null && <AnsweredStatus {...status} waiting={meta.state === "waiting"} />}
            {/* A late event's "Update: …" line: small, under the answer, never a new answer. */}
            {updates.map((index) => (
              <div
                key={`update:${index}`}
                data-slot="answer-update"
                className="text-foreground/80 border-foreground/15 mt-activity border-s-2 ps-3 text-sm leading-relaxed wrap-break-word"
              >
                <ReplyText index={index} streaming={false} />
              </div>
            ))}
          </>
        )}
      </div>
      </StoppedBy.Provider>
      {/* A run's work is merged from its card, never undone behind the run's back. */}
      {!live && meta.session && !isRunRequest(meta.requestId) && <TurnDiff requestId={meta.requestId} />}
      {!live && answer !== null && <AnswerFilesOf index={answer} requestIds={meta.requestIds} />}
      {!meta.session && <TurnMemories requestIds={meta.requestIds} />}
      <BlockError meta={meta} live={live} />
      {!live && last >= 0 && !phase && (
        <AnswerActions
          session={meta.session}
          rework={meta.rework}
          atMs={meta.endedAtMs}
          shown={answer === null ? null : [answer, ...updates]}
          forkId={meta.forkId}
        />
      )}
      <ContinuedFrom forkId={meta.forkId} />
      {meta.compactions
        .filter((compaction) => !compaction.inTurn)
        .map((compaction) => (
          <CompactionBlock key={compaction.id} compaction={compaction} />
        ))}
    </MessagePrimitive.Root>
  );
};

/**
 * Whether a finished turn's work is open. It folds by default; the thread keeps the header in
 * place on screen while the work opens or closes under it (see `thread-scroll.ts`).
 */
function useFold() {
  const [open, setOpen] = useState(false);
  const show = useCallback(() => setOpen(true), []);
  return { open, toggle: () => setOpen(!open), show };
}

/** A turn's work cut at the user's follow-ups: runs of work, and the steered bubbles between them. */
type WorkPart = { type: "work"; items: Activity<LeadStep, Entry>[] } | { type: "steer"; entry: Entry };

/**
 * The work in runs between its steered bubbles. A live turn always ends on a run, where its live
 * line goes, even right after a bubble.
 */
function workParts(items: readonly Activity<LeadStep, Entry>[], live: boolean): WorkPart[] {
  const parts: WorkPart[] = [];
  for (const item of items) {
    if (item.type === "entry" && item.entry.kind === "steer") {
      parts.push({ type: "steer", entry: item.entry });
      continue;
    }
    const last = parts.at(-1);
    if (last?.type === "work") last.items.push(item);
    else parts.push({ type: "work", items: [item] });
  }
  if (live && parts.at(-1)?.type !== "work") parts.push({ type: "work", items: [] });
  return parts;
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
 * Under the answer: copy it, read it aloud, rate it and, in a Chat, ask for another answer and move between
 * answers (a session's answers have neither). The latest answer always shows them, an older one
 * on hover or focus; when it came shows on hover. Copy and Read aloud take the text `shown`
 * (the answer and its update lines, by part), or else the last part.
 */
const AnswerActions: FC<{
  session: boolean;
  rework: boolean;
  atMs: number | null;
  shown: number[] | null;
  forkId: string | null;
}> = ({ session, rework, atMs, shown, forkId }) => {
  const conversation = useViewConversation();
  const answer = useAuiState((s) => {
    const parts = s.message.parts;
    return (shown ?? [parts.length - 1])
      .map((index) => parts[index])
      .map((part) => (part?.type === "text" ? part.text : ""))
      .filter((text) => text !== "")
      .join("\n\n");
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
      data-slot="answer-actions"
      autohide="never"
      className={cn(
        // 26px buttons 2px apart, their 16px icons at half white.
        "text-foreground/50 gap-answer-actions-between -ms-1 flex items-center [&_.aui-button-icon]:size-answer-action",
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
      <ReadAloud text={answer} />
      <RateMenu rated={rated}>
        <ActionBarPrimitive.FeedbackPositive asChild>
          <RateItem rating="good" />
        </ActionBarPrimitive.FeedbackPositive>
        <ActionBarPrimitive.FeedbackNegative asChild>
          <RateItem rating="bad" />
        </ActionBarPrimitive.FeedbackNegative>
      </RateMenu>
      {!session && rework && (
        <ActionBarPrimitive.Reload asChild>
          <TooltipIconButton tooltip="Try again">
            <Regenerate />
          </TooltipIconButton>
        </ActionBarPrimitive.Reload>
      )}
      {!session && <BranchPicker />}
      {conversation && forkId && (
        <ForkMenu conversationId={conversation.id} kind={conversation.kind} messageId={forkId} />
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
const ContinuedFrom: FC<{ forkId: string | null }> = ({ forkId }) => {
  const origin = useViewConversation()?.forkedFrom ?? null;
  const source = useApp((s) => (origin ? s.conversations[origin.conversationId] : undefined));
  if (!origin || !forkId || origin.messageId !== forkId) return null;
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
