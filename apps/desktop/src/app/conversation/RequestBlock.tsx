import { MessagePrimitive, useAuiState, jumpInThread, useThreadAnswerStarted, useThreadHoldsOnFold } from "@/components/conversation";
import { type FC, Fragment, type RefObject, useCallback, useEffect, useRef, useState } from "react";

import { ActivityItems, TrailingLine } from "@/app/conversation/ActivityItems";
import { AnswerActions } from "@/app/conversation/AnswerActions";
import { BlockError, useBlockQuotaRecovery } from "@/app/conversation/BlockError";
import { leadTransitions, transitionCoversSeam } from "@/app/conversation/providerTransitions";
import { CompactionBlock } from "@/app/conversation/CompactionBlock";
import { SeamDivider } from "@/app/conversation/SeamRows";
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
import { useBlockHold, useLiveWait } from "@/app/conversation/blockHold";
import { useFindSearching } from "@/app/conversation/ThreadFind";
import { WorkFold } from "@/app/conversation/WorkFold";
import { turnParts, type WorkPart } from "@/app/conversation/workParts";
import { reopened } from "@/app/conversation/turnWorkers";
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
  blockFinished,
  blockSequence,
  foldsAway,
  foldsWork,
  isLive,
  liveAnswer,
  sentMessageIds,
  type SequenceEntry as Entry,
  updateIndices,
} from "@/app/conversation/blocks";
import type { ThreadStatusProps } from "@/app/conversation/ThreadStatus";
import { useTurnChanged } from "@/app/conversation/ChangesCard";
import { useViewConversation } from "@/app/conversation/viewContext";
import type { AttachmentRef, ModelChoice, ThinkingSegment, WorkSpan } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useOpenTurnStart } from "@/state/cliThread";
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
    /** It still streams. */
    live?: boolean;
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
  /** Quiet lines of what its workers did, and of background jobs' ends its CLI took up, by position. */
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
 * right now. Once its final answer starts (or, with a CLI that doesn't say, once it is done),
 * everything before the answer folds into "Worked for 3m 4s"; the cards that matter (decisions,
 * failures, what needs the user), the user's follow-ups and what comes after the answer while
 * the turn still runs stay in view. It doesn't fold by itself while subagents or workers the
 * turn started still work; the user's own choice, kept per turn, wins over both.
 */
/** A run of work's key: the cut it follows, so it keeps its row whatever comes into it or before that cut. */
function partKey(parts: readonly WorkPart[], index: number): string {
  const cut = parts.slice(0, index).findLast((part) => part.type === "between");
  return cut?.type === "between" ? `work:after:${entryKey(cut.entry)}` : "work:first";
}

export const RequestBlock: FC = () => {
  const meta = useAuiState((s) => s.message.metadata.custom["block"]) as BlockMeta | undefined;
  const fold = useFold(meta?.requestId ?? null);
  // While Find searches, closed work keeps its rows, hidden, to be found.
  const searched = useFindSearching();
  const body = useRef<HTMLDivElement>(null);
  const answerSlot = useRef<HTMLDivElement>(null);
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
      jumpInThread(declined, "center");
      setRevealDenied(false);
    }, 50);
    return () => window.clearTimeout(timer);
  }, [revealDenied]);
  const conversationId = useViewConversation()?.id ?? null;
  // A jump to a message this turn sent opens its work.
  const revealing = useRevealing(conversationId);
  const holds = !!revealing && !!meta?.orchestratorSteps.some((step) => sentMessageIds(step.kind).includes(revealing));
  const { show } = fold;
  useEffect(() => {
    if (holds) show();
  }, [holds, show]);
  const { wait: quotaWait, recovered: quotaRecovered } = useBlockQuotaRecovery(meta);
  const requestIds = meta?.requestIds;
  const changed = useTurnChanged(requestIds ?? []);
  // Its model's turn runs now: what that turn offers waits for it to end, not for its workers.
  const turnRuns = useBoard((s) => {
    const board = s.board;
    return !quotaWait && !!board && (board.run === "running" || board.run === "starting") && !!board.runRequest && !!requestIds?.includes(board.runRequest);
  });
  // What its turns started that still works holds it open, with no end cards yet, and its work
  // doesn't fold by itself meanwhile (D24).
  const { workers: holding, commands, subagents, held, heldUntil, named } = useBlockHold(conversationId, meta);
  const wait = useLiveWait(conversationId);
  const openStart = useOpenTurnStart(conversationId);
  // Its cards load before its work can open, so they are there the moment it does.
  const hasCards = (meta?.cards.length ?? 0) > 0;
  useEffect(() => {
    if (hasCards) void loadCardBody();
  }, [hasCards]);
  if (!meta) return null;

  const again = reopened(openStart, meta.startedAtMs, meta.endedAtMs);
  const live = isLive(meta.state) || again || held || quotaWait !== null || (quotaRecovered && turnRuns);
  const last = meta.texts.length - 1;
  const sequence = blockSequence(meta);
  // Its CLI's turn is over and nothing it started still works: only then does its work fold, the
  // same moment on every CLI (D21, D24).
  const done = blockFinished(meta.state) && !again && !held && quotaWait === null && !(quotaRecovered && turnRuns);
  const transitions = leadTransitions(meta.orchestratorSteps);
  // The answer: once done, its last reply; until then the reply it is writing at its end, in the
  // same place, so the answer never moves when the work folds (an answer a step then follows
  // goes back among the steps).
  const answer = done ? answerIndex(meta.texts, true) : liveAnswer(meta.texts, sequence);
  const updates = done ? updateIndices(meta.texts, answer) : [];
  const activity = turnActivity(done ? sequence : sequence.filter((entry) => entry.kind !== "text" || entry.index !== answer), false);
  // The answer, kept cards and access notices stay outside the fold.
  const kept = done ? sequence.filter((entry) => entry.kind !== "text" && !foldsAway(entry, answer)) : [];
  const allParts = turnParts(activity, answer, done, live, (index) => {
    const text = meta.texts[index];
    return !!text && !text.commentary && !text.update;
  });
  // Bubbles the user sent before any work came sit under their message, above the header.
  const leading = allParts.findIndex((part) => part.type !== "between");
  const before = allParts.slice(0, leading < 0 ? allParts.length : leading);
  const parts = allParts.slice(before.length);
  const foldable = foldsWork(done, answer, parts.some((part) => part.type === "work" && part.items.length > 0));
  const open = fold.open ?? false;
  const header =
    foldable ||
    meta.state === "stopped" ||
    meta.state === "failed" ||
    // A Chat's says it works over its steps, and with none until its reply streams, once something came;
    // done, it stays as "Worked for", so ending moves nothing.
    ((live || done) && (meta.session || meta.steers.length > 0 || activity.some((item) => item.type === "group" && item.items.some((each) => each.type === "step")) || (meta.texts.length === 0 && sequence.length > 0)));
  const streams = (index: number) => !!meta.texts[index]?.live;
  const streaming = (entry: Entry) => !quotaWait && !done && entry.kind === "text" && streams(entry.index);
  const commentary = (entry: Entry) => entry.kind === "text" && !!meta.texts[entry.index]?.commentary;
  const liveThought = quotaWait ? undefined : sequence.findLast((entry) => entry.kind === "thinking" && entry.live);
  const lastPart = parts.at(-1);
  const liveWork = live && !done && lastPart?.type === "work" ? lastPart : null;
  const status: ThreadStatusProps = {
    requestIds: meta.requestIds,
    state: isLive(meta.state) ? meta.state : "working",
    thinkingLive: !!liveThought,
    compacting: meta.compactions.some((compaction) => compaction.inTurn && compaction.state === "running"),
    quotaWait,
    wait,
    // Over, its line says what it waits on: its workers, then its commands; a subagent's own row says it runs (D96).
    ...(held && { workers: holding, commands, subagents, named }),
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
      <ModelChanged model={meta.texts[last]?.model ?? null} picked={meta.picked} transitions={transitions} />
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
            meta={meta.state === "failed" && (quotaWait || quotaRecovered)
              ? { ...meta, state: quotaWait ? "waiting" : turnRuns ? "working" : "done" } : meta}
            open={open}
            foldable={foldable}
            quota={quotaWait !== null}
            // Over, it works on while what it started runs (L3, D24).
            held={held && meta.state === "done"}
            heldUntil={heldUntil}
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
            <Fragment key={partKey(parts, index)}>
            <WorkFold open={!foldable || open} clicked={fold.clicked} searched={searched}>
              <div
                data-slot="request-work"
                className={cn("gap-activity flex min-w-0 flex-col", (header || index > 0) && "pt-activity")}
              >
                <ActivityItems
                  activity={part.items}
                  requestIds={meta.requestIds}
                  live={liveWork === part ? status : null}
                  working={live && !done}
                  thought={liveThought?.kind === "thinking" ? liveThought.segment.text : null}
                  streaming={streaming}
                  commentary={commentary}
                  // A follow-up steered in after it: the work before it is over, whatever still runs.
                  summary={parts.slice(index + 1).some((next) => next.type === "between")}
                  revealDenied={revealDenied}
                  // The live line goes under the answer (TrailingLine), so it never moves past it.
                  lineBelow={false}
                  answered={liveWork === part && answer !== null}
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
        {kept.map((entry) => (
          <div key={entryKey(entry)} className="pt-activity flex min-w-0 flex-col">
            <SequenceEntry entry={entry} streaming={false} />
          </div>
        ))}
        {answer !== null && (
          <div
            ref={answerSlot}
            data-slot="aui_assistant-message-content"
            data-answer=""
            data-commentary={(!done && meta.texts[answer]?.commentary) || undefined}
            // The replies Brigadier sent other sessions with this answer land a jump here.
            data-peer-message={meta.texts[answer]?.anchors?.join(" ")}
            className={cn("text-foreground leading-relaxed wrap-break-word", (header || parts.length > 0 || before.length > 0) && "pt-activity")}
          >
            <AnswerDetails.Provider value={changed}>
              <ReplyText index={answer} streaming={!quotaWait && !done && streams(answer)} report={done} />
            </AnswerDetails.Provider>
          </div>
        )}
        {liveWork && <TrailingLine activity={liveWork.items} live={status} thought={liveThought?.kind === "thinking" ? liveThought.segment.text : null} answered={answer !== null} />}
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
      </div>
      </TurnEnded.Provider>
      </StoppedBy.Provider>
      {!live && <MadeImages steps={meta.orchestratorSteps} />}
      {!live && <TurnEnd meta={meta} />}
      <BlockError meta={meta} live={live} quotaCovered={quotaWait !== null || quotaRecovered} />
      {answer !== null && (
        <AnswerAnnouncer answering={live} finished={!live} index={answer} />
      )}
      <AnswerStarted requestIds={meta.requestIds} started={done} answer={answerSlot} folded={foldable} />
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
      {meta.seams.filter((seam) => !transitions.some((transition) => transitionCoversSeam(transition, seam))).map((seam) => (
        <SeamDivider key={seam.n} seam={seam} />
      ))}
    </MessagePrimitive.Root>
  );
};

/** No turns. */

/** The user's open or closed choice per turn, by request: kept while the app runs, through a thread switch. */
const folds = new Map<string, boolean>();

/**
 * Whether a finished turn's work is open: as the user last left it (`undefined` while they never
 * chose, for the turn's default), and whether that was their click on its header (`clicked`),
 * which alone animates it opening. The thread keeps the header in place on screen while the
 * work opens or closes under it (see `thread-scroll.ts`).
 */
function useFold(requestId: string | null) {
  const [state, setState] = useState<{ open: boolean | undefined; clicked: boolean }>(() => ({
    open: requestId === null ? undefined : folds.get(requestId),
    clicked: false,
  }));
  const set = useCallback((next: boolean, clicked = true) => {
    if (requestId !== null) folds.set(requestId, next);
    setState({ open: next, clicked });
  }, [requestId]);
  // Opened for a jump to what it holds: no click's.
  const show = useCallback(() => set(true, false), [set]);
  return { open: state.open, clicked: state.clicked, set, show };
}

/**
 * Tells the thread when the turn's final answer starts (see `useThreadAnswerStarted`), and keeps
 * the answer where it shows while the work folds by itself above it (`folded`).
 */
const AnswerStarted: FC<{ requestIds: readonly string[]; started: boolean; answer: RefObject<HTMLDivElement | null>; folded: boolean }> = ({
  requestIds,
  started,
  answer,
  folded,
}) => {
  useThreadAnswerStarted(requestIds, started);
  useThreadHoldsOnFold(answer, folded);
  return null;
};

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
