import { MessagePrimitive, useAuiState, jumpInThread } from "@/components/conversation";
import { type FC, Fragment, useCallback, useEffect, useRef, useState } from "react";

import { ActivityItems } from "@/app/conversation/ActivityItems";
import { AnswerActions } from "@/app/conversation/AnswerActions";
import { BlockError } from "@/app/conversation/BlockError";
import { CompactionBlock } from "@/app/conversation/CompactionBlock";
import { ContinuedFrom } from "@/app/conversation/ContinuedFrom";
import { JoinedAnswer } from "@/app/conversation/JoinedAnswer";
import { JoinedAnswerActions } from "@/app/conversation/JoinedAnswerActions";
import { ModelChanged } from "@/app/conversation/ModelChanged";
import { ReplyText } from "@/app/conversation/ReplyText";
import { AnswerDetails } from "@/app/conversation/ReportText";
import { entryKey, loadCardBody, SequenceEntry } from "@/app/conversation/SequenceEntry";
import { TurnEnd } from "@/app/conversation/TurnEnd";
import { UpdateText } from "@/app/conversation/UpdateText";
import { WorkHeader } from "@/app/conversation/WorkHeader";
import { useFindSearching } from "@/app/conversation/ThreadFind";
import { WorkFold } from "@/app/conversation/WorkFold";
import { turnParts } from "@/app/conversation/workParts";
import { MadeImages } from "@/app/conversation/activity/ImageRows";
import { TurnEnded } from "@/app/conversation/FeedbackOffer";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { turnActivity } from "@/app/conversation/activity/group";
import { deniedCall } from "@/app/conversation/activity/words";
import {
  type BlockCard,
  type BlockCompaction,
  type BlockSeam,
  type BlockTaskNotice,
  type BlockOrchestratorStep,
  type BlockRow,
  type BlockState,
  type PeerOf,
  answerIndex,
  answeringIndex,
  blockFinished,
  blockSequence,
  foldsAway,
  isFinal,
  isLive,
  reopenedSince,
  type SequenceEntry as Entry,
  updateIndices,
} from "@/app/conversation/blocks";
import { AnsweredStatus, type ThreadStatusProps } from "@/app/conversation/ThreadStatus";
import { useTurnChanged } from "@/app/conversation/ChangesCard";
import { useViewConversation } from "@/app/conversation/viewContext";
import type { AttachmentRef, ModelChoice, ThinkingSegment, WorkSpan } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useRevealing } from "@/state/reveal";

/** What an assistant block carries in its message metadata (`custom.block`). */
export type BlockMeta = {
  /** Per text part, in order: the reply's position (infinite while it streams) and model. */
  texts: {
    /** The reply's own message: what its rating is kept on. */
    messageId: string;
    /** When it was written. */
    atMs?: number;
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
  /** Seams after it (G12). */
  seams: BlockSeam[];
  /** Brigadier's seam message in its turn, by position. */
  nudges: number[];
  /** The CLI's lines for background jobs that ended, by position. */
  taskNotices: BlockTaskNotice[];
  state: BlockState;
  startedAtMs: number;
  endedAtMs: number | null;
  /** When its turn worked, for "Worked for …". */
  worked: WorkSpan[];
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
  /** It and the requests steered into it. */
  requestIds: string[];
  /** The message the user rates: the block's answer, not an update line under it. */
  answerId: string | null;
  /** The message a fork starts from: the block's last reply, an update line included. */
  forkId: string | null;
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
  const fold = useFold(meta?.requestId ?? null);
  // While Find searches, closed work keeps its rows, hidden, to be found.
  const searched = useFindSearching();
  const body = useRef<HTMLDivElement>(null);
  // The calls of its own the user denied, and the header's ask to show them.
  const deniedCount = useBoard((s) => {
    const approvals = Object.values(s.board?.approvals ?? {});
    if (!meta || approvals.length === 0) return 0;
    return meta.orchestratorSteps.filter((step) => step.kind.type === "tool" && deniedCall(step.kind, step.atMs, meta.requestIds, approvals)).length;
  });
  const [revealDenied, setRevealDenied] = useState(false);
  useEffect(() => {
    if (!revealDenied) return;
    // Once the work and the groups holding them have opened: the first in view, then ready for another click.
    const timer = window.setTimeout(() => {
      const declined = body.current?.querySelector("[data-step-state=declined]");
      jumpInThread(declined);
      declined?.scrollIntoView({ block: "center" });
      setRevealDenied(false);
    }, 50);
    return () => window.clearTimeout(timer);
  }, [revealDenied]);
  // A jump to a message this turn sent opens its work.
  const revealing = useRevealing(useViewConversation()?.id ?? null);
  const holds = !!revealing && !!meta?.orchestratorSteps.some((step) => step.kind.type === "peerSent" && step.kind.messageIds.includes(revealing));
  const { show } = fold;
  useEffect(() => {
    if (holds) show();
  }, [holds, show]);
  const quotaWait = useViewConversation()?.quotaWait ?? null;
  const requestIds = meta?.requestIds;
  const changed = useTurnChanged(requestIds ?? []);
  const workersActive = useBoard((s) =>
    Object.values(s.board?.tasks ?? {}).some(
      (task) => task.requestId !== null && !!requestIds?.includes(task.requestId) && !isFinal(task),
    ),
  );
  // Its model's turn runs now: what that turn offers waits for it to end, not for its workers.
  const turnRuns = useBoard((s) => {
    const board = s.board;
    return !!board && (board.run === "running" || board.run === "starting") && !!board.runRequest && !!requestIds?.includes(board.runRequest);
  });
  // Its cards load before its work can open, so they are there the moment it does.
  const hasCards = (meta?.cards.length ?? 0) > 0;
  useEffect(() => {
    if (hasCards) void loadCardBody();
  }, [hasCards]);
  if (!meta) return null;

  const live = isLive(meta.state);
  const last = meta.texts.length - 1;
  const sequence = blockSequence(meta);
  const finished = blockFinished(meta.state);
  // Its work folds the moment its CLI starts the final answer, though the request may still wait
  // (for another session's reply, say); with a CLI that doesn't say, once the request is done.
  // Until then a reply streams in place.
  const answering = !finished && live && !workersActive ? answeringIndex(meta.texts, sequence) : null;
  const done = finished || answering !== null;
  const answer = answering ?? answerIndex(meta.texts, finished);
  const answerAtMs = answer === null ? undefined : meta.texts[answer]?.answerAtMs;
  const updates = updateIndices(meta.texts, answer);
  // Decide which peer send is still in flight where the live row state is known.
  const activity = turnActivity(sequence, false);
  // The answer, kept cards and access notices stay outside the fold.
  const kept = sequence.filter((entry) => entry.kind !== "text" && !foldsAway(entry, answer));
  const allParts = turnParts(activity, answer, done, live, (index) => {
    const text = meta.texts[index];
    return !!text && !text.commentary && !text.update;
  });
  // Bubbles the user sent before any work came sit under their message, above the header.
  const leading = allParts.findIndex((part) => part.type !== "between");
  const before = allParts.slice(0, leading < 0 ? allParts.length : leading);
  const parts = allParts.slice(before.length);
  const foldable = done && parts.some((part) => part.type === "work" && part.items.length > 0);
  const open = fold.open ?? false;
  const header =
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
    state: isLive(meta.state) ? meta.state : "working",
    thinkingLive: !!liveThought,
    compacting: meta.compactions.some((compaction) => compaction.inTurn && compaction.state === "running"),
    quotaWait,
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
      <ModelChanged model={meta.texts[last]?.model ?? null} picked={meta.picked} />
      {/* A step still running when the user stopped the turn reads "Stopped …". */}
      <StoppedBy.Provider value={meta.state === "stopped" ? { atMs: meta.endedAtMs } : null}>
      {/* A feedback offer waits for the turn that made it to end: never mid-turn (#486). */}
      <TurnEnded.Provider value={!live || !turnRuns}>
      <div ref={body} data-slot="request-body" className="flex min-w-0 flex-col">
        {before.length > 0 && (
          <div data-slot="request-leading-steers" className="gap-activity pb-activity flex min-w-0 flex-col">
            {before.map((part) => part.type === "between" && <SequenceEntry key={entryKey(part.entry)} entry={part.entry} streaming={false} />)}
          </div>
        )}
        {header && (
          <WorkHeader
            meta={meta}
            open={open}
            foldable={foldable}
            quota={quotaWait !== null && meta.state === "working" && !workersActive}
            answering={answering !== null}
            answerAtMs={answerAtMs}
            onToggle={() => fold.set(!open)}
            denied={deniedCount}
            onDenied={() => {
              fold.set(true);
              setRevealDenied(true);
            }}
          />
        )}
        {/* The work, cut at the user's follow-ups: each run of it folds on its own, and the
            bubbles between them stay mounted whether it is open or folded, so nothing below
            them pops in or out when it settles (THREAD-PARITY-PLAN.md §4.2). */}
        {parts.map((part, index) =>
          part.type === "between" ? (
            <div key={entryKey(part.entry)} className={cn("flex min-w-0 flex-col", (header || index > 0) && "pt-activity")}>
              <SequenceEntry entry={part.entry} streaming={false} />
            </div>
          ) : (
            <Fragment key={`work:${index}`}>
            <WorkFold open={!done || open} searched={searched}>
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
                  summary={parts.slice(index + 1).some((next) => next.type === "between")}
                  revealDenied={revealDenied}
                />
              </div>
            </WorkFold>
            {/* The answer its request came to before a follow-up joined it: in view, as the last one's is. */}
            {part.answer && (
              <JoinedAnswer>
                <SequenceEntry entry={part.answer} streaming={false} />
                {part.answer.kind === "text" && meta.texts[part.answer.index] && (
                  <JoinedAnswerActions index={part.answer.index} text={meta.texts[part.answer.index]!} />
                )}
              </JoinedAnswer>
            )}
            </Fragment>
          ),
        )}
        {done && (
          <>
            {kept.map((entry) => (
              <div key={entryKey(entry)} className="pt-activity flex min-w-0 flex-col">
                <SequenceEntry entry={entry} streaming={false} />
              </div>
            ))}
            {answer !== null && (
              <div
                data-slot="aui_assistant-message-content"
                data-answer=""
                // The replies Brigadier sent other sessions with this answer land a jump here.
                data-peer-message={meta.texts[answer]?.anchors?.join(" ")}
                className={cn("text-foreground leading-relaxed wrap-break-word", (header || parts.length > 0 || before.length > 0) && "pt-activity")}
              >
                <AnswerDetails.Provider value={changed}>
                  <ReplyText index={answer} streaming={streams(answer)} report={!streams(answer)} />
                </AnswerDetails.Provider>
              </div>
            )}
            {/* Answered, it may still wait for another session's reply: that line stays under the answer. */}
            {answering !== null && <AnsweredStatus {...status} waiting={meta.state === "waiting"} reopened={meta.state === "working" && reopenedSince(meta.worked, answerAtMs)} />}
            {/* A late event's note: a plain short paragraph under the answer, never a new answer. */}
            {updates.map((index) => (
              <div
                key={`update:${index}`}
                data-slot="answer-update"
                className="text-foreground mt-activity leading-relaxed wrap-break-word"
              >
                <MessagePrimitive.PartByIndex index={index} components={{ Text: UpdateText }} />
              </div>
            ))}
          </>
        )}
      </div>
      </TurnEnded.Provider>
      </StoppedBy.Provider>
      {!live && <MadeImages steps={meta.orchestratorSteps} />}
      {!live && <TurnEnd meta={meta} />}
      <BlockError meta={meta} live={live} />
      {answer !== null && (
        <AnswerAnnouncer answering={live} finished={!live} index={answer} />
      )}
      {!live && last >= 0 && (
        <AnswerActions
          session={meta.session}
          rework={meta.rework}
          atMs={meta.endedAtMs}
          shown={answer === null ? null : [answer, ...updates]}
          forkId={meta.forkId}
          answerId={meta.answerId}
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

/** The user's open or closed choice per turn, by request: kept while the app runs, through a thread switch. */
const folds = new Map<string, boolean>();

/**
 * Whether a finished turn's work is open: as the user last left it (`undefined` while they never
 * chose, for the turn's default). The thread keeps the header in place on screen while the work
 * opens or closes under it (see `thread-scroll.ts`).
 */
function useFold(requestId: string | null) {
  const [open, setOpen] = useState<boolean | undefined>(() => (requestId === null ? undefined : folds.get(requestId)));
  const set = useCallback((next: boolean) => {
    if (requestId !== null) folds.set(requestId, next);
    setOpen(next);
  }, [requestId]);
  const show = useCallback(() => set(true), [set]);
  return { open, set, show };
}

/**
 * Says the turn's answer to a screen reader: "Response started" when it starts, then "Response
 * complete" with its text. Only for the turn it sees happen, never for a thread's history.
 */
const AnswerAnnouncer: FC<{ answering: boolean; finished: boolean; index: number }> = ({ answering, finished, index }) => {
  const text = useAuiState((s) => {
    const part = s.message.parts[index];
    return part?.type === "text" ? part.text : "";
  });
  const [said, setSaid] = useState<{ started: boolean; words: string }>({ started: false, words: "" });
  if (answering && !finished && !said.started) setSaid({ started: true, words: "Response started" });
  if (finished && said.started && !said.words.startsWith("Response complete")) setSaid({ started: true, words: `Response complete: ${text}` });
  return (
    <div role="status" aria-live="polite" className="sr-only" data-slot="answer-announcer">
      {said.words}
    </div>
  );
};
