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
  Regenerate,
  SoundOnReadOutLoudSpeaker,
  Stop,
  TextShorterConcise,
} from "@openai/apps-sdk-ui/components/Icon";
import { type FC, lazy, Suspense, useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { ThinkingRow } from "@/app/conversation/ThinkingRow";
import { WorkFold } from "@/app/conversation/WorkFold";
import { ForkMenu } from "@/app/conversation/ForkMenu";
import { InlineImageText } from "@/app/conversation/InlineImage";
import { MentionText } from "@/app/conversation/Mentions";
import { ThreadStep } from "@/app/conversation/activity/Notice";
import { ActivityGroup } from "@/app/conversation/activity/ActivityGroup";
import { type Activity, type LeadStep, turnActivity } from "@/app/conversation/activity/group";
import { describeLeadStep, LeadStepRow } from "@/app/conversation/activity/LeadStep";
import { ROW } from "@/components/assistant-ui/elements/activity-row";
import {
  type BlockCard,
  type BlockCompaction,
  type BlockOrchestratorStep,
  type BlockRow,
  type BlockState,
  answerIndex,
  blockSequence,
  foldsAway,
  isFinal,
  isLive,
  isRunRequest,
  type SequenceEntry as Entry,
  turnTime,
  waitsOnCard,
} from "@/app/conversation/blocks";
import { type PhaseView, phaseViewOf, splitReport } from "@/app/conversation/phaseView";
import { TeamSentence } from "@/app/conversation/activity/TeamSentence";
import { ThreadStatus } from "@/app/conversation/ThreadStatus";
import { TurnDiff } from "@/app/conversation/TurnDiff";
import { TurnMemories } from "@/app/conversation/TurnMemories";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ErrorState } from "@/components/assistant-ui/elements/error-state";
import {
  BranchPicker,
  MarkdownBlock,
  MessageText,
  StreamingMessageText,
} from "@/components/assistant-ui/thread";
import { preserveAnchor } from "@/components/assistant-ui/preserve-anchor";
import { RateItem, RateMenu } from "@/components/assistant-ui/rate-menu";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import type { AttachmentRef, ModelChoice, ThinkingSegment, WorkSpan } from "@/ipc/generated";
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
  thinking?: ThinkingSegment[];
  /** Messages the user steered into the turn, shown as bubbles in the work. */
  steers: { position: number; text: string; atMs: number; attachments: AttachmentRef[] }[];
  /** Compactions of a Chat's context in the turn, or after it. */
  compactions: BlockCompaction[];
  state: BlockState;
  startedAtMs: number;
  endedAtMs: number | null;
  /** When its requests worked, for "Worked for …". */
  worked: WorkSpan[];
  /** It waits only for quota, not for the user. */
  quotaWait: boolean;
  /** It waits for the answer to its own question card, so it still works. */
  cardWait: boolean;
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

function headerLabel(state: BlockState, elapsed: number, quota = false): string {
  const time = formatDuration(elapsed);
  switch (state) {
    case "working":
      if (quota) return `Waiting for quota · ${time}`;
      return elapsed < 1000 ? "Working" : `Working for ${time}`;
    case "waiting":
      return quota ? `Waiting for quota · ${time}` : `Waiting for you · ${time}`;
    case "done":
      return `Worked for ${time}`;
    case "stopped":
      return `You stopped after ${time}`;
    case "failed":
      return `Failed after ${time}`;
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

/** A live turn shows no header until it has worked this long (only "Thinking"). */
const HEADER_AFTER_MS = 2000;

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
  /** Called with the header, before the fold opens or closes. */
  onToggle: (header: HTMLElement) => void;
}> = ({ meta, phase, open, foldable, quota, onToggle }) => {
  const phaseElapsed = useElapsed(phase?.startedAtMs ?? 0, phase?.endedAtMs ?? null, !!phase && !phase.settled);
  const now = useTicking(!phase && isLive(meta.state));
  const elapsed = phase ? phaseElapsed : turnTime(meta, now);
  // Waiting on its own question card, the turn still works: the card says it waits.
  const state = waitsOnCard(meta) ? "working" : meta.state;
  const label = phase ? phaseLabel(phase, elapsed) : headerLabel(state, elapsed, quota || meta.quotaWait);
  const text = (
    <span
      data-slot="request-work-label"
      className={cn(
        "text-sm leading-(--spacing-activity-row) tabular-nums",
        meta.state === "failed" && !phase ? "text-destructive" : "text-foreground/50",
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
      onClick={(event) => onToggle(event.currentTarget)}
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
  return (
    <div data-slot="request-card">
      <Suspense fallback={null}>
        <CardBody type={card.type} id={card.id} />
      </Suspense>
    </div>
  );
}

/** One reply of the block; text that still streams fades in word by word. */
const ReplyText: FC<{ index: number; streaming: boolean; report?: boolean }> = ({ index, streaming, report = false }) => (
  <MessagePrimitive.PartByIndex
    index={index}
    components={{ Text: report ? ReportText : streaming ? StreamingMessageText : MessageText }}
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
      <details data-slot="report-details">
        <summary className="text-muted-foreground rounded-control cursor-pointer text-sm">
          Details
        </summary>
        <div className="pt-2">
          <MarkdownBlock text={report.details} />
        </div>
      </details>
    </div>
  );
};

/** A reply, card or row in the block's work. */
const SequenceEntry: FC<{ entry: Entry; streaming: boolean }> = ({ entry, streaming }) => {
  switch (entry.kind) {
    case "thinking":
      // Only the live thought is an entry: a settled one sits in its work group.
      return entry.live ? <ThinkingRow text={entry.segment.text} live /> : null;
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
      return <SteerBubble text={entry.text} atMs={entry.atMs} attachments={entry.attachments} />;
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
      {live && elapsed >= HEADER_AFTER_MS && (
        <div className="border-border flex items-center border-b pb-2">
          <span className="text-muted-foreground text-sm">{headerLabel("working", elapsed)}</span>
        </div>
      )}
      <CompactionRow compaction={compaction} />
    </div>
  );
};

/** A follow-up the user sent while the block worked: their bubble, inside the block. */
const SteerBubble: FC<{ text: string; atMs: number; attachments: readonly AttachmentRef[] }> = ({
  text,
  atMs,
  attachments,
}) => {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  return (
    <div
      data-slot="request-steer"
      className="group/steer flex max-w-7/10 min-w-0 flex-col items-end gap-y-1 self-end"
    >
      <div className="bg-muted text-foreground rounded-bubble max-w-full min-w-0 px-4 py-2.5 whitespace-pre-wrap wrap-anywhere">
        <InlineImageText text={text} attachments={attachments} Text={MentionText} />
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
 * A turn's activity in order (THREAD-UX-PLAN.md §3.1): its work groups and everything else as it
 * is. The live and the folded turn render the same items, so nothing moves when the turn ends;
 * only its last group, while the turn is live, says the step it is on.
 */
const ActivityItems: FC<{
  activity: readonly Activity<LeadStep, Entry>[];
  live: boolean;
  streaming: (entry: Entry) => boolean;
}> = ({ activity, live, streaming }) => {
  const tasks = useBoard((s) => s.board?.tasks);
  return (
    <>
      {activity.map((item, index) =>
        item.type === "group" ? (
          <ActivityGroup
            key={`group:${item.key}`}
            items={item.items}
            live={live && index === activity.length - 1}
            describe={(step) => describeLeadStep(step, tasks)}
            renderStep={(step, key) => <LeadStepRow key={key} step={step} />}
          />
        ) : (
          <SequenceEntry key={entryKey(item.entry)} entry={item.entry} streaming={streaming(item.entry)} />
        ),
      )}
    </>
  );
};

/** The block's error: what went wrong in full, and Try again where a Chat can answer again. */
const BlockError: FC<{ retry: boolean }> = ({ retry }) => {
  const error = useAuiState((s) => {
    const status = s.message.status;
    return status?.type === "incomplete" && status.reason === "error"
      ? String(status.error ?? "The reply failed.")
      : null;
  });
  const aui = useAui();
  if (error === null) return null;
  return (
    <ErrorState
      title="Something went wrong"
      detail={error}
      onRetry={retry ? () => aui.message().reload() : undefined}
    />
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
  const work = meta.rows.length > 0 || meta.orchestratorSteps.length > 0 || meta.cards.length > 0 || (meta.thinking?.length ?? 0) > 0;
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
    useShallow((s) => (meta && s.board && isRunRequest(meta.requestId) ? phaseViewOf(s.board.overnight, s.board.plans, meta.requestId) : null)),
  );
  const workersActive = useBoard((s) =>
    Object.values(s.board?.tasks ?? {}).some(
      (task) => task.requestId !== null && !!requestIds?.includes(task.requestId) && !isFinal(task),
    ),
  );
  // A run's morning report folds its details.
  const report = useBoard((s) => {
    const id = meta?.answerId;
    return !!id && Object.values(s.board?.overnight ?? {}).some((run) => run.reportMessageId === id);
  });
  if (!meta) return null;

  // A run's block is live until the run is over, whatever still waits on the user: that waits in the panel.
  const live = phase ? !phase.settled : isLive(meta.state);
  const last = meta.texts.length - 1;
  const sequence = blockSequence(meta);
  // The final answer is streaming: the workers it waited for are all over, or a session's thread
  // writes after its work (it writes nothing between its steps but one opening line, before them).
  // The work folds now, when the final answer starts.
  const streamingAt = sequence.findIndex((entry) => entry.kind === "text" && entry.index === last);
  const workBefore = sequence.slice(0, Math.max(0, streamingAt)).some((entry) => entry.kind === "orchestrator" || entry.kind === "row" || entry.kind === "card");
  const answering =
    meta.state === "working" &&
    (meta.rows.length > 0 || (meta.session && workBefore)) &&
    !workersActive &&
    meta.texts[last]?.position === Number.POSITIVE_INFINITY;
  const done = phase ? phase.settled : meta.state === "done" || answering;
  // A run over folds to its outcome; the thread's replies during it go into the fold.
  const answer = answerIndex(meta.texts, done && !phase);
  const activity = turnActivity(sequence);
  // The answer and the cards that stay in view are outside the fold.
  const folded = activity.filter((item) => item.type === "group" || foldsAway(item.entry, answer));
  const kept = meta.cards.filter((card) => card.keep);
  const parts = workParts(done ? folded : activity, live && !done);
  const foldable = done && parts.some((part) => part.type === "work" && part.items.length > 0);
  const outcome = done ? (phase?.outcome ?? null) : null;
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
      className="group/answer gap-answer-actions-gap relative flex flex-col px-2"
    >
      {/* A run picks its models itself: not a change the user made. */}
      {!phase && <ModelChanged model={meta.texts[last]?.model ?? null} picked={meta.picked} />}
      <div data-slot="request-body" className="flex min-w-0 flex-col">
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
                data-follow-content={live && index === parts.length - 1 ? "" : undefined}
                className={cn("gap-activity flex min-w-0 flex-col", (header || index > 0) && "pt-activity")}
              >
                <ActivityItems
                  activity={part.items}
                  live={live && !done && index === parts.length - 1}
                  streaming={(entry) => !done && entry.kind === "text" && meta.texts[entry.index]?.position === Number.POSITIVE_INFINITY}
                />
                {live && !done && index === parts.length - 1 && (
                  <>
                    {sequence.map((entry) => entry.kind === "thinking" && entry.live && <SequenceEntry key={entryKey(entry)} entry={entry} streaming={false} />)}
                    <ThreadStatus
                      requestIds={meta.requestIds}
                      // A live run works on, whatever the thread's last turn came to.
                      state={isLive(meta.state) ? meta.state : "working"}
                      thinkingLive={sequence.some((entry) => entry.kind === "thinking" && entry.live)}
                      compacting={meta.compactions.some((compaction) => compaction.inTurn && compaction.state === "running")}
                      quotaWait={quotaWait}
                    />
                  </>
                )}
              </div>
            </WorkFold>
          ),
        )}
        {done && (
          <>
            {kept.map((card) => (
              <div key={`${card.type}:${card.id}`} className="pt-activity flex min-w-0 flex-col">
                <CardEntry card={card} />
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
                className={cn("text-foreground leading-relaxed wrap-break-word", (header || parts.length > 0) && "pt-activity")}
              >
                <ReplyText index={answer} streaming={answering} report={(report || meta.session) && !answering} />
              </div>
            )}
          </>
        )}
      </div>
      {/* A run's work is merged from its card, never undone behind the run's back. */}
      {!live && meta.session && !isRunRequest(meta.requestId) && <TurnDiff requestId={meta.requestId} />}
      {!meta.session && <TurnMemories requestIds={meta.requestIds} />}
      <BlockError retry={!meta.session && meta.rework} />
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

/**
 * Whether a finished turn's work is open. It folds by default, and the header keeps its place on
 * screen while the work opens or closes under it.
 */
function useFold() {
  const [open, setOpen] = useState(false);
  const toggle = (header: HTMLElement) => {
    preserveAnchor(header);
    setOpen(!open);
  };
  return { open, toggle };
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
