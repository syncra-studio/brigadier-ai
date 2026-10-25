import { toolName } from "@/app/conversation/activity/words";
import type { CardType } from "@/app/conversation/cards/CardBody";
import type {
  AttachmentRef,
  Compaction,
  CompactionState,
  Decision,
  MachineStep,
  MachineStepKind,
  Message,
  ItemStatus,
  ModelChoice,
  OrchestratorStep,
  OrchestratorStepKind,
  PeerMessage,
  QueuedMessage,
  ProviderKind,
  Question,
  RequestState,
  Task,
  ThinkingSegment,
  WorkerStep,
  WorkerStepKind,
  WorkSpan,
} from "@/ipc/generated";
import { SEPARATOR_GAP_MS } from "@/lib/format";
import type { PendingMessage } from "@/state/store";

/**
 * One user request as the thread shows it: the user's message, then one assistant block with
 * everything the request set in motion (replies, workers, cards), however it interleaved with
 * other requests in the conversation's stream. Requests group by the id the daemon stores on
 * each item, so a reload, a restart or an older page groups exactly the same way.
 */

/** A card or worker in a block, in the order it appeared. */
export type BlockCard = {
  type: CardType;
  id: string;
  position: number;
  /** Shown outside the fold: a decision, a failure, or something that needs the user. */
  keep: boolean;
};

/** A reply of the block, as a text part of the assistant message (by index). */
export type BlockText = {
  messageId: string;
  position: number;
  text: string;
  model: ModelChoice | null;
  /** A short "Update: …" line after the answer, for a late event (SESSION-FIXES-PLAN.md bug 4). */
  update: boolean;
  /**
   * The replies Brigadier sent other sessions carrying this answer: a jump to one of them lands
   * here (SESSION-MESSAGING-PLAN.md §6.3).
   */
  anchors?: readonly string[];
  /** Its CLI said it is the turn's final answer: when it started (the work folds from there). */
  answerAtMs?: number;
  /** It streams, and its CLI said it is a line between the turn's steps. */
  commentary?: boolean;
  /** When it was written (a reply still streaming: when it started). */
  atMs?: number;
};

/**
 * Whether a reply is an update line: it starts with "Update:", bold or not ("**Update**:" too),
 * and is one short paragraph. An ending that opens with "Update:" (paragraphs, a Details section)
 * is the answer, never a line under an earlier one (#231).
 */
export function isUpdate(text: string): boolean {
  const body = text.trim();
  return /^(\*\*|__)?update(\*\*|__)?:/i.test(body) && !/\n[ \t]*\n/.test(body) && !/^#{1,6}[ \t]/m.test(body);
}

/** A late note's words without its "Update:" marker, first letter up: the marker is for the app. */
export function withoutUpdateMark(text: string): string {
  const body = text.trim().replace(/^(\*\*|__)?update(\*\*|__)?:(\*\*|__)?[ \t]*/i, "");
  return body ? `${body.charAt(0).toUpperCase()}${body.slice(1)}` : text;
}

/**
 * A row of a block's work that updates in place: a worker's task, with its landing folded into
 * it. An older task's checks open from its row; an older overnight phase's checks are workers
 * like any other.
 */
/** `wave`: a start whose workers have all come to the same later word, said by the start's own row. */
export type BlockRow = { type: "task"; taskId: string; taskIds?: string[]; kind?: WorkerStepKind; position: number; wave?: boolean };

/**
 * A judgement call made on the user's behalf, shown as a quiet row among the orchestrator's.
 * Routine outcomes of a task or a plan (landed, sent back, held) show on its own row instead.
 */
export type DecidedStep = { type: "decided"; what: string; why: string };

/** A wait of the thread for the replies to its messages, as its call stands. */
export type PeerWaitCall = { itemId: string; status: ItemStatus; requestId: string | null };

/**
 * Messages this session sent others, and its waits for their replies, as one row of the work
 * (SESSION-MESSAGING-PLAN.md §6.1): "Sent message to Docs, waited for its reply". The records
 * say where each went and how it stands; adjacent sends and waits share the row.
 */
export type PeerSentStep = { type: "peerSent"; messageIds: readonly string[]; waits: readonly PeerWaitCall[] };

/** A row about the machine: work waiting for it to cool down, a build paused or resumed. */
export type MachineWords = {
  type: "machine";
  machine: MachineStepKind;
  reason: MachineStep["reason"];
  command: string | null;
  taskId: string | null;
};

/**
 * Something the orchestrator did ("Sent message to …"), or Brigadier decided for the user, in
 * the order it happened.
 */
export type BlockOrchestratorStep = {
  kind: OrchestratorStepKind | DecidedStep | MachineWords | PeerSentStep;
  position: number;
  /** When the step began (a tool call: when it was first seen). */
  atMs?: number;
  /** The request it served: a tool call's row reads its call with the request's calls. */
  requestId?: string | null;
};

/**
 * A Chat's model compacting its context ("Compacting context" → "Context compacted"): in the
 * turn it happened in (`inTurn`, the model compacted on its own), or after the block it
 * followed (the user asked for it between turns).
 */
export type BlockCompaction = {
  id: string;
  inTurn: boolean;
  automatic: boolean;
  state: CompactionState["type"];
  error: string | null;
  position: number;
  startedAtMs: number;
  endedAtMs: number | null;
};

/**
 * A seam after the block (G12): the session went on in a fresh context, from a hand-off note
 * the thread wrote (openable), or with none (`death` says why).
 */
export type BlockSeam = {
  n: number;
  note: string | null;
  start: string | null;
  death: string | null;
  /** The other CLI it continued on, as asked (S38c). */
  movedTo: ProviderKind | null;
};

/**
 * What of a seam the thread shows: a hand-off is under the hood (run2-D44), its note and a
 * missing one alike; only the other CLI the thread went on in, which now answers.
 */
export function seamMove(seam: BlockSeam): ProviderKind | null {
  return seam.death ? null : seam.movedTo;
}

/**
 * A message steered into the block's running turn: the user's bubble, or another session's
 * message block (a message steered in, or a reply to the request), inside the block.
 */
export type BlockSteer = {
  message: Message;
  text: string;
  position: number;
};

/**
 * Another session's message that opened a request: its block in the input position. `earlier`
 * marks a reply to a request older than the newest, shown at its own place in the thread
 * ("to your earlier message").
 */
export type PeerAnchor = { kind: "peer"; message: Message; text: string; earlier: boolean };

/**
 * Where a message from another session came from, as its block shows it: who sent it, and
 * whether it answers one of this session's ("Reply from ‹title›"), an earlier request's
 * included ("· to your earlier message").
 */
export type PeerOf = { messageId: string; from: string; reply: boolean; earlier: boolean };

/** Whether a received reply went to a wait of the thread's turn, as its result, and stays there. */
export function heldForWait(record: PeerMessage | undefined): boolean {
  return record?.hold?.type === "handed" || record?.hold?.type === "taken";
}

/** A transcript message's sender, when another session sent it. */
export function peerOf(message: Message, earlier = false): PeerOf | undefined {
  if (message.role !== "peer" || !message.peer) return undefined;
  return { messageId: message.id, from: message.peer.from, reply: message.peer.replyTo !== undefined, earlier };
}

/** The message a block's request started with: the user's, or another session's. */
export function anchorMessage(block: Pick<Block, "user">): Message | null {
  return block.user?.kind === "message" || block.user?.kind === "peer" ? block.user.message : null;
}

export type BlockState = RequestState["type"];

/**
 * A quiet line of what a worker did (Brigadier's message of the workers' news), or of a
 * background job's end the CLI took up, where it came.
 */
export type BlockTaskNotice = {
  position: number;
  text: string;
  /** The CLI's own words of it, on hover, when the line says them shorter. */
  detail?: string;
  /** It came with a turn that joined the block that started its worker: a line in the work, folding with it. */
  inWork?: boolean;
};

export type Block = {
  key: string;
  /**
   * The message it answers: the user's, or another session's; absent for replies older than any
   * loaded user message.
   */
  user: { kind: "message"; message: Message; text: string } | { kind: "pending"; pending: PendingMessage } | PeerAnchor | null;
  texts: BlockText[];
  cards: BlockCard[];
  /** Workers the request started, by task number. */
  tasks: string[];
  /** Its workers' rows, in the order they started. */
  rows: BlockRow[];
  /** The orchestrator's steps, in order. */
  orchestratorSteps: BlockOrchestratorStep[];
  thinking: ThinkingSegment[];
  /** Messages steered into its turn, whose requests it shows too. */
  steers: BlockSteer[];
  /** Compactions in its turn, or after it. */
  compactions: BlockCompaction[];
  /** Seams after it: a fresh context took over from here. */
  seams: BlockSeam[];
  /** Brigadier's one message of a seam in its turn ("write your hand-off note"), where it came. */
  nudges: number[];
  /** Brigadier's quiet lines of what its workers did ("Worker "…" finished · …"), where they came. */
  taskNotices: BlockTaskNotice[];
  /** The requests it shows: its own (the key), then the steered ones. */
  requestIds: string[];
  state: BlockState;
  error: string | null;
  startedAtMs: number;
  endedAtMs: number | null;
  /** When its turn worked, by its CLI's timestamps (open while it does), for "Worked for …". */
  worked: WorkSpan[];
  /** It waits for the answer to its own question card. */
  cardWait: boolean;
};

/** The parts of the board a thread's turns show beside the CLI's items (`boardItems`). */
export type BoardDigest = {
  tasks: Readonly<Record<string, Task>>;
  workerSteps?: readonly WorkerStep[];
  questions: Readonly<Record<string, Question>>;
  orchestratorSteps: readonly OrchestratorStep[];
  machineSteps: readonly MachineStep[];
  decisions: readonly Decision[];
  compactions: Readonly<Record<string, Compaction>>;
  /** Messages to and from other sessions, by message id. */
  peerMessages?: Readonly<Record<string, PeerMessage>>;
  /** The queue rail's messages: one the CLI already has shows there, not in the thread. */
  queueItems?: readonly QueuedMessage[];
};

const WORKING: ReadonlySet<Task["state"]> = new Set([
  "queued",
  "starting",
  "running",
  "blocked",
  "landing",
]);

const FINAL: ReadonlySet<Task["state"]> = new Set(["landed", "done", "rejected", "stopped", "failed"]);


/** How long the spans worked in all, overlaps counted once; an open span works until `now`. */
export function workedMs(spans: readonly WorkSpan[], now: number): number {
  let total = 0;
  let reach = Number.NEGATIVE_INFINITY;
  for (const span of spans.toSorted((a, b) => a.fromMs - b.fromMs)) {
    const to = Math.max(span.fromMs, span.toMs ?? now);
    const from = Math.max(span.fromMs, reach);
    if (to > from) total += to - from;
    reach = Math.max(reach, to);
  }
  return total;
}

/**
 * When the spans last stopped working, or `null` when none has. A span still open belongs to a
 * follow-up that waits for quota, not to the wait for the user.
 */
export function stoppedAtMs(spans: readonly WorkSpan[]): number | null {
  const stops = spans.flatMap((span) => (span.toMs === null ? [] : [span.toMs]));
  return stops.length ? Math.max(...stops) : null;
}

/** Whether a waiting turn waits only for the answer to its question card: it still reads as working. */
export function waitsOnCard(meta: { state: BlockState; cardWait: boolean }): boolean {
  return meta.state === "waiting" && meta.cardWait;
}

/**
 * The time a turn shows: while it works, waits for quota or for the answer to its question
 * card, how long it worked so far; while it waits for the user otherwise, how long it has
 * waited; once over, how long it worked, its waits for the user left out ([`waitedTime`]
 * gives its card waits).
 */
export function turnTime(
  meta: { state: BlockState; worked: readonly WorkSpan[]; cardWait: boolean; endedAtMs: number | null },
  now: number,
  /** When its final answer started: it worked until then, whatever came after. */
  answerAtMs?: number,
): number {
  if (answerAtMs !== undefined) {
    const until = (at: number) => Math.min(at, answerAtMs);
    const spans = meta.worked
      .filter((span) => span.fromMs < answerAtMs)
      .map((span) => ({ ...span, toMs: until(span.toMs ?? answerAtMs) }));
    return workedMs(spans, until(now));
  }
  if (meta.state === "waiting" && !waitsOnCard(meta)) {
    return Math.max(0, now - (stoppedAtMs(meta.worked) ?? meta.endedAtMs ?? now));
  }
  return workedMs(meta.worked, now);
}

/** Whether a task is over: its worker is gone and it will not run again. */
export function isFinal(task: Task): boolean {
  return FINAL.has(task.state);
}

/**
 * Steps a team sentence already says: a worker created is its "started working", an accepted
 * change its "Landed …", a report read its "finished".
 */
const ON_TASK_ROW: ReadonlySet<OrchestratorStepKind["type"]> = new Set(["created", "accepted", "readReport"]);

/** Steps of old records whose cards and rows are gone. */
const OLD_RECORDS: ReadonlySet<OrchestratorStepKind["type"]> = new Set([
  "fullAccessSuggested",
  "settingSuggested",
  "sessionOffered",
  "sessionOfferTaken",
  "ciWatch",
]);

/** What a worker's lifecycle event is called; adjacent events that share a word merge into one sentence. */
export function lifecycleWord(kind: WorkerStepKind | undefined): "started" | "waiting" | "stopped" | "failed" | "finished" {
  switch (kind) {
    case "started":
    case "resumed":
    case undefined:
      return "started";
    case "waiting":
    case "paused":
      return "waiting";
    case "stopped":
    case "rejected":
      return "stopped";
    case "failed":
      return "failed";
    default:
      return "finished";
  }
}

/** Whether a decision is a judgement call the thread shows, rather than a task's, plan's or phase's routine outcome. */
export function judgementCall(decision: Decision): boolean {
  // An answer to a worker shows as its "Answered …" line.
  if (decision.kind === "answer") return false;
  if (decision.source.type === "orchestrator") return true;
  // A phase's outcome ("Verified phase 1 “Measure”") is said by the phase's header already.
  return decision.source.type === "run" && decision.kind !== "phaseOutcome";
}

/** An overnight run's own request (or its report's): the thread's turns, tasks and reports during the run. */
export function isRunRequest(requestId: string | null): boolean {
  return requestId?.startsWith("run-") ?? false;
}

/**
 * Something of Brigadier's own a turn shows beside its CLI's items (`cliBlocks`): a card, a
 * worker, a worker's row, a row of its own (a merge, a message to another session, the
 * machine, a decision), or a compaction. Each names the request it belongs to, and when it
 * began, which places it among the CLI's items.
 */
export type BoardItem =
  | { kind: "card"; requestId: string | null; card: BlockCard; atMs: number }
  | { kind: "task"; requestId: string | null; id: string; atMs: number }
  | { kind: "row"; requestId: string | null; row: BlockRow; atMs: number }
  | { kind: "orchestrator"; requestId: string | null; step: BlockOrchestratorStep; atMs: number }
  | { kind: "compaction"; requestId: string | null; after: string | null; compaction: BlockCompaction; atMs: number };

/**
 * What the board holds that the CLI's session file doesn't, as items of the thread's turns, in
 * the order they came. The CLI's own calls are its file's: none of them comes from here.
 */
export function boardItems(board: BoardDigest): BoardItem[] {
  const items: (BoardItem & { position: number })[] = [];
  for (const task of Object.values(board.tasks)) {
    items.push({ kind: "task", position: task.position, requestId: task.requestId, id: task.id, atMs: task.createdAtMs });
    // Old conversations without lifecycle events still have a quiet start line.
    if (!(board.workerSteps ?? []).some((step) => step.taskId === task.id && step.kind === "started")) {
      items.push({ kind: "row", position: task.position, requestId: task.requestId,
        row: { type: "task", taskId: task.id, kind: "started", position: task.position },
        atMs: task.createdAtMs });
    }
  }
  // A worker the lead stopped: its "Stopped …" row says so, with the reason.
  const stoppedByLead = new Set(
    board.orchestratorSteps.flatMap((step) => (step.kind.type === "stopped" ? [step.kind.taskId] : [])),
  );
  for (const step of board.workerSteps ?? []) {
    if (!board.tasks[step.taskId] || ["updated", "landed"].includes(step.kind)) continue;
    if (step.kind === "stopped" && stoppedByLead.has(step.taskId)) continue;
    items.push({ kind: "row", position: step.position, requestId: step.requestId,
      row: { type: "task", taskId: step.taskId, kind: step.kind, position: step.position },
      atMs: step.atMs });
  }
  for (const step of board.orchestratorSteps) {
    // The CLI's calls are in its file.
    if (step.kind.type === "tool") continue;
    // The user's answer to a feedback offer shows on the offer itself.
    if (step.kind.type === "feedbackAnswered") continue;
    // Old records of what Brigadier no longer does: not shown. A suggestion of Full access (from
    // when it had a sandbox of its own), a suggested setting, an offer to start a session and its
    // answer, a CI watch.
    if (OLD_RECORDS.has(step.kind.type)) continue;
    // A run's own steps show as its phases, but a refused message, a usage limit's reset or
    // switch and a proof the thread attached have no other trace.
    if (ON_TASK_ROW.has(step.kind.type) || (isRunRequest(step.requestId) && step.kind.type !== "messageRefused" && step.kind.type !== "quotaResumed" && step.kind.type !== "moment" && step.kind.type !== "proof")) continue;
    items.push({
      kind: "orchestrator",
      position: step.position,
      requestId: step.requestId,
      step: { kind: step.kind, position: step.position, atMs: step.atMs, requestId: step.requestId },
      atMs: step.atMs,
    });
  }
  // A message to another session is a row of the work that sent it, from its record. The answer
  // Brigadier sent back for this session is its turn's answer itself: no row.
  for (const peer of Object.values(board.peerMessages ?? {})) {
    if (!peer.outgoing || peer.automatic || peer.sentAtMs == null) continue;
    items.push({
      kind: "orchestrator",
      position: peer.position,
      requestId: peer.requestId,
      step: { kind: { type: "peerSent", messageIds: [peer.messageId], waits: [] }, position: peer.position, atMs: peer.sentAtMs },
      atMs: peer.sentAtMs,
    });
  }
  for (const step of board.machineSteps) {
    if (step.taskId !== null) continue;
    items.push({
      kind: "orchestrator",
      position: step.position,
      requestId: step.requestId,
      step: {
        kind: { type: "machine", machine: step.kind, reason: step.reason, command: step.command, taskId: step.taskId },
        position: step.position,
      },
      atMs: step.atMs,
    });
  }
  for (const decision of board.decisions) {
    if (!judgementCall(decision)) continue;
    items.push({
      kind: "orchestrator",
      position: decision.position,
      requestId: decision.requestId,
      step: {
        kind: { type: "decided", what: decision.what, why: decision.why },
        position: decision.position,
      },
      atMs: decision.atMs,
    });
  }
  for (const compaction of Object.values(board.compactions)) {
    items.push({
      kind: "compaction",
      position: compaction.position,
      requestId: compaction.requestId,
      after: compaction.after,
      compaction: {
        id: compaction.id,
        inTurn: compaction.requestId !== null,
        automatic: compaction.automatic,
        state: compaction.state.type,
        error: compaction.state.type === "failed" ? compaction.state.error : null,
        position: compaction.position,
        startedAtMs: compaction.startedAtMs,
        endedAtMs: compaction.endedAtMs,
      },
      atMs: compaction.startedAtMs,
    });
  }
  // Approvals wait in the composer's place and leave nothing in the thread once answered. A
  // question card is a row of the work, which folds with it once the request is done.
  for (const question of Object.values(board.questions)) {
    items.push({
      kind: "card",
      position: question.position,
      requestId: question.requestId,
      card: { type: "question", id: question.id, position: question.position, keep: false },
      atMs: question.createdAtMs,
    });
  }
  return items.toSorted((a, b) => a.position - b.position).map(({ position: _position, ...item }) => item);
}

/** What a block's work is made of, as its message metadata carries it. */
export type SequenceSource = {
  texts: readonly { position: number; anchors?: readonly string[]; answerAtMs?: number; atMs?: number }[];
  cards: readonly BlockCard[];
  steers: readonly { position: number; text: string; atMs: number; attachments: readonly AttachmentRef[]; requestId?: string; peer?: PeerOf }[];
  compactions: readonly BlockCompaction[];
  nudges?: readonly number[];
  taskNotices?: readonly BlockTaskNotice[];
  orchestratorSteps: readonly BlockOrchestratorStep[];
  thinking?: readonly ThinkingSegment[];
  state?: BlockState;
  rows: readonly BlockRow[];
};

/** One line or card of a block's work. */
export type SequenceEntry =
  | { kind: "thinking"; segment: ThinkingSegment; live: boolean; position: number }
  | { kind: "text"; index: number; position: number; anchors?: readonly string[] }
  | { kind: "card"; card: BlockCard; position: number }
  | { kind: "steer"; text: string; atMs: number; attachments: readonly AttachmentRef[]; position: number; requestId?: string; peer?: PeerOf }
  | { kind: "orchestrator"; steps: BlockOrchestratorStep[]; position: number }
  | { kind: "compaction"; compaction: BlockCompaction; position: number }
  /** Brigadier asked the thread for its hand-off note (G12): its line, not a user's bubble. */
  | { kind: "nudge"; position: number }
  /** A quiet line of what a worker did ("Worker "…" finished · …"). */
  | { kind: "taskNotice"; text: string; position: number; inWork?: boolean; detail?: string }
  | { kind: "row"; row: BlockRow; position: number }
  /** More than an hour passed in the turn before what follows, which came at `atMs` (#130). */
  | { kind: "gap"; atMs: number; position: number };

/** Whether a lead's step is an image the thread looked at or made: its row stands apart from the work. */
export function isImageStep(step: BlockOrchestratorStep): boolean {
  return step.kind.type === "tool" && step.kind.item === "image";
}

/** Whether a lead's image step made the image (rather than looked at it). */
export function madeImage(step: BlockOrchestratorStep): boolean {
  return step.kind.type === "tool" && toolName(step.kind.name) === "imagegen";
}

/** Whether an entry is only images the thread looked at (or only images it made): one row says them all. */
function imageEntry(entry: SequenceEntry): "viewed" | "made" | null {
  if (entry.kind !== "orchestrator" || !entry.steps.every(isImageStep)) return null;
  if (entry.steps.every(madeImage)) return "made";
  return entry.steps.some(madeImage) ? null : "viewed";
}

/**
 * The block's replies, cards, rows and orchestrator steps in order, one entry each; adjacent
 * "started working" rows share one sentence, adjacent images the thread looked at one row
 * ("Viewed 3 images"). The lead's work steps are grouped by `activity/group.ts`.
 */
export function blockSequence(source: SequenceSource): SequenceEntry[] {
  const entries: SequenceEntry[] = [
    ...(source.thinking ?? []).map((segment) => ({ kind: "thinking" as const, segment, live: false, position: segment.position })),
    ...source.texts.map((text, index) => ({
      kind: "text" as const,
      index,
      position: text.position,
      ...(text.anchors && { anchors: text.anchors }),
    })),
    ...source.cards.map((card) => ({ kind: "card" as const, card, position: card.position })),
    ...source.steers.map((steer) => ({ kind: "steer" as const, ...steer })),
    ...source.compactions
      .filter((compaction) => compaction.inTurn)
      .map((compaction) => ({ kind: "compaction" as const, compaction, position: compaction.position })),
    ...(source.nudges ?? []).map((position) => ({ kind: "nudge" as const, position })),
    ...(source.taskNotices ?? []).map((notice) => ({ kind: "taskNotice" as const, ...notice })),
    // A proof shows with the merge message, under the answer, not in the work.
    ...source.orchestratorSteps.filter((step) => step.kind.type !== "proof").map((step) => ({
      kind: "orchestrator" as const,
      steps: [step],
      position: step.position,
    })),
    ...source.rows.map((row) => ({ kind: "row" as const, row, position: row.position })),
  ].toSorted((a, b) => a.position - b.position);
  const tail = entries.at(-1);
  if (tail?.kind === "thinking") tail.live = !tail.segment.complete && (source.state === undefined || source.state === "working");
  const merged: SequenceEntry[] = [];
  for (const entry of entries) {
    const previous = merged.at(-1);
    if (entry.kind === "row" && previous?.kind === "row" && lifecycleWord(entry.row.kind) === lifecycleWord(previous.row.kind)) {
      previous.row = { ...previous.row, taskIds: [...new Set([...(previous.row.taskIds ?? [previous.row.taskId]), entry.row.taskId])] };
      continue;
    }
    const images = imageEntry(entry);
    if (images && previous?.kind === "orchestrator" && entry.kind === "orchestrator" && imageEntry(previous) === images) {
      previous.steps = [...previous.steps, ...entry.steps];
      continue;
    }
    merged.push(entry);
  }
  return gaps(waves(merged), source.texts);
}

/** When an entry began and when it last did something, as far as the thread knows. */
function entryTimes(entry: SequenceEntry, texts: SequenceSource["texts"]): { from?: number | undefined; to?: number | undefined } {
  switch (entry.kind) {
    case "thinking":
      return { from: entry.segment.startedAtMs, to: entry.segment.updatedAtMs };
    case "text": {
      const text = texts[entry.index];
      return { from: text?.answerAtMs ?? text?.atMs, to: text?.atMs };
    }
    case "steer":
      return { from: entry.atMs, to: entry.atMs };
    case "orchestrator": {
      const ends = entry.steps.map((step) => (step.kind.type === "tool" ? step.kind.endedAtMs : undefined) ?? step.atMs);
      return { from: entry.steps[0]?.atMs, to: ends.every((end) => end !== undefined) ? Math.max(...ends) : undefined };
    }
    case "compaction":
      return { from: entry.compaction.startedAtMs, to: entry.compaction.endedAtMs ?? entry.compaction.startedAtMs };
    default:
      // Cards and team sentences carry no time of their own here.
      return {};
  }
}

/**
 * A "time passed" line inside the turn: between two things the thread did more than an hour
 * apart, as above a user's message sent more than an hour after the one before it (#130).
 */
function gaps(entries: SequenceEntry[], texts: SequenceSource["texts"]): SequenceEntry[] {
  const out: SequenceEntry[] = [];
  let reached: number | undefined;
  for (const entry of entries) {
    const { from, to } = entryTimes(entry, texts);
    // Where a usage limit reset, its own divider says when the work went on, not a time line (#128).
    if (entry.kind === "orchestrator" && entry.steps.some((step) => step.kind.type === "quotaResumed")) {
      if (from !== undefined) reached = Math.max(reached ?? from, from);
      out.push(entry);
      continue;
    }
    if (from !== undefined && reached !== undefined && from - reached > SEPARATOR_GAP_MS) {
      out.push({ kind: "gap", atMs: from, position: entry.position });
    }
    const latest = to ?? from;
    if (latest !== undefined) reached = Math.max(reached ?? latest, latest);
    out.push(entry);
  }
  return out;
}

/**
 * Workers started together are one row: once every one of them has come to the same later word
 * ("finished"), the start's row says it ("Count math and Count strings finished") and the rows
 * that said it of each go. Until then each later word has its own row.
 */
function waves(entries: SequenceEntry[]): SequenceEntry[] {
  const gone = new Set<SequenceEntry>();
  entries.forEach((start, index) => {
    if (start.kind !== "row" || lifecycleWord(start.row.kind) !== "started") return;
    const wave = new Set(start.row.taskIds ?? [start.row.taskId]);
    const later = entries.slice(index + 1).filter((entry): entry is Extract<SequenceEntry, { kind: "row" }> =>
      entry.kind === "row" && !gone.has(entry) && lifecycleWord(entry.row.kind) !== "started"
      && (entry.row.taskIds ?? [entry.row.taskId]).every((id) => wave.has(id)));
    // The newest word every worker of the wave came to.
    const last = later.at(-1);
    if (!last) return;
    const word = lifecycleWord(last.row.kind);
    const same = later.filter((entry) => lifecycleWord(entry.row.kind) === word);
    const reached = new Set(same.flatMap((entry) => entry.row.taskIds ?? [entry.row.taskId]));
    if (reached.size !== wave.size) return;
    start.row = { ...start.row, ...(last.row.kind && { kind: last.row.kind }), wave: true };
    for (const entry of same) gone.add(entry);
  });
  return entries.filter((entry) => !gone.has(entry));
}

/**
 * Whether a block is finished, so its work folds under "Worked for …" with an answer below it:
 * once its request is done. A working request shows its
 * replies in place, also one written after its work with no worker left: Brigadier may still
 * wake its thread (a review, a held merge, a CI result), and a line written mid-way ("Diff looks
 * right.") is no answer.
 */
export function blockFinished(state: BlockState): boolean {
  return state === "done";
}

/**
 * A finished block's answer, by its index in `texts`: the request's last reply that isn't an
 * update line, so an ending written again after a late fix replaces the earlier one in place,
 * which folds into the work (THREAD-PARITY-PLAN §4.6), while a late event's "Update: …" line
 * shows under the answer and leaves it in view (SESSION-FIXES-PLAN.md bug 4). `null` while it
 * works, or with no reply.
 */
export function answerIndex(texts: readonly { update?: boolean }[], done: boolean): number | null {
  if (!done || texts.length === 0) return null;
  const at = texts.findLastIndex((text) => !text.update);
  return at >= 0 ? at : texts.length - 1;
}

/**
 * The reply a block still at work answers with: the one its CLI said is the turn's final answer,
 * while it is the newest thing in the work. Its work folds the moment it starts and the answer
 * goes under "Worked for …", though the request may still wait (for another session's reply,
 * say). `null` otherwise, always with a CLI that doesn't say: its work folds once it is done.
 */
export function answeringIndex(texts: readonly { answerAtMs?: number; update?: boolean }[], sequence: readonly SequenceEntry[]): number | null {
  const newest = sequence.findLast((entry) => entry.kind !== "thinking");
  if (newest?.kind !== "text") return null;
  // A late event's "Update: …" line goes under the answer it follows, which stays the answer.
  const answer = texts.findLastIndex((text) => !text.update);
  const at = texts[newest.index]?.update && answer >= 0 ? answer : newest.index;
  return texts[at]?.answerAtMs === undefined ? null : at;
}

/**
 * Whether a request works again since its answer started at `answerAtMs`: a late result (a
 * failed full check, say) reopened it, so its answer is no longer what it is doing (#107).
 */
export function reopenedSince(worked: readonly WorkSpan[], answerAtMs: number | undefined): boolean {
  const last = worked.at(-1);
  return answerAtMs !== undefined && last !== undefined && last.toMs === null && last.fromMs > answerAtMs;
}

/**
 * An answer split at its `### Details` heading: what shows, and the details the thread folds
 * (the checks run, workers and models, provider usage). Null when it has none.
 */
export function splitReport(text: string): { head: string; details: string } | null {
  // The heading as a model writes it too: `## Details`, `#### Details`, `### Details:`.
  const at = text.search(/^#{2,4}[ \t]+Details:?[ \t]*$/m);
  const end = at < 0 ? -1 : text.indexOf("\n", at);
  if (end < 0) return null;
  const details = text.slice(end + 1).trim();
  return details ? { head: text.slice(0, at).trimEnd(), details } : null;
}

/** The update lines under a finished block's answer: the replies after it, by index. */
export function updateIndices(texts: readonly unknown[], answer: number | null): number[] {
  return answer === null ? [] : texts.map((_, index) => index).filter((index) => index > answer);
}

/**
 * Whether an entry of a finished block folds under "Worked for …": everything but its answer
 * and the cards and feedback offers that stay in view (decisions, failures, what
 * needs the user), and the dividers of its limits and switches (#506).
 */
export function foldsAway(entry: SequenceEntry, answer: number | null): boolean {
  // The answer, and the update lines after it, stay in view.
  if (entry.kind === "text") return answer === null || entry.index < answer;
  if (entry.kind === "orchestrator" && entry.steps.some((step) => step.kind.type === "feedbackOffered" || step.kind.type === "moment")) return false;
  return entry.kind !== "card" || !entry.card.keep;
}

/** Whether a block still has work running or waiting. */
export function isLive(state: BlockState): boolean {
  return state === "working" || state === "waiting";
}

/** Whether a task still runs (for chips). */
export function isWorking(task: Task): boolean {
  return WORKING.has(task.state);
}

/** Whether a block has anything to show: a finished block with nothing in it hides. */
export function blockShows(block: Block): boolean {
  return (
    block.texts.length > 0 ||
    block.cards.length > 0 ||
    block.tasks.length > 0 ||
    block.orchestratorSteps.length > 0 ||
    block.thinking.length > 0 ||
    block.compactions.length > 0 ||
    block.taskNotices.length > 0 ||
    block.state !== "done"
  );
}

/**
 * A message of the thread as assistant-ui sees it: a user message or a request's block, under
 * its parent. Siblings (an edit beside the message it replaced, another answer beside the one
 * it replaced) share a parent.
 */
export type ThreadNode = {
  id: string;
  parentId: string | null;
  kind: "user" | "block";
  block: Block;
  /** The stored message the branch would end at if this node were the last one shown. */
  head: string | null;
};

export type ThreadTree = {
  nodes: ThreadNode[];
  /** The last node of the branch shown. */
  headId: string | null;
};
