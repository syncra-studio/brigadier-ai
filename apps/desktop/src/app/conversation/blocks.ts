import type { CardType } from "@/app/conversation/cards/CardBody";
import type {
  Approval,
  Compaction,
  CompactionState,
  Decision,
  Message,
  ModelChoice,
  OrchestratorStep,
  OrchestratorStepKind,
  Plan,
  Question,
  RequestState,
  Task,
  UserRequest,
} from "@/ipc/generated";
import type { Board } from "@/state/board";
import { shownIdOf } from "@/state/shownIds";
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
};

/**
 * A row of a block's work that updates in place: a worker's task, with its checks, fixes and
 * landing folded into it, or the whole-phase checks of an overnight phase. Checkers never get a
 * row of their own; they open from the row of what they check.
 */
export type BlockRow =
  | { type: "task"; taskId: string; position: number }
  | { type: "phaseChecks"; runId: string; phaseId: string; position: number };

/**
 * A judgement call made on the user's behalf, shown as a quiet row among the orchestrator's.
 * Routine outcomes of a task or a plan (landed, sent back, held) show on its own row instead.
 */
export type DecidedStep = { type: "decided"; what: string; why: string };

/**
 * Something the orchestrator did ("Sent message to …"), or Brigadier decided for the user, in
 * the order it happened.
 */
export type BlockOrchestratorStep = {
  kind: OrchestratorStepKind | DecidedStep;
  position: number;
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

/** A message the user steered into the block's running turn: a bubble inside the block. */
export type BlockSteer = {
  message: Message;
  text: string;
  position: number;
};

export type BlockState = RequestState["type"];

export type Block = {
  key: string;
  /** The user's message; absent for replies older than any loaded user message. */
  user: { kind: "message"; message: Message; text: string } | { kind: "pending"; pending: PendingMessage } | null;
  texts: BlockText[];
  cards: BlockCard[];
  /** Workers the request started, by task number. */
  tasks: string[];
  /** Its workers' rows, in the order they started. */
  rows: BlockRow[];
  /** The orchestrator's steps, in order. */
  orchestratorSteps: BlockOrchestratorStep[];
  /** Messages steered into its turn, whose requests it shows too. */
  steers: BlockSteer[];
  /** Compactions in its turn, or after it. */
  compactions: BlockCompaction[];
  /** The requests it shows: its own (the key), then the steered ones. */
  requestIds: string[];
  state: BlockState;
  error: string | null;
  startedAtMs: number;
  endedAtMs: number | null;
};

/** The parts of the board the blocks depend on (not worker activity or transcripts). */
export type BoardDigest = {
  tasks: Readonly<Record<string, Task>>;
  approvals: Readonly<Record<string, Approval>>;
  questions: Readonly<Record<string, Question>>;
  plans: Readonly<Record<string, Plan>>;
  requests: Readonly<Record<string, UserRequest>>;
  orchestratorSteps: readonly OrchestratorStep[];
  decisions: readonly Decision[];
  compactions: Readonly<Record<string, Compaction>>;
  runRequest: string | null;
  streaming: Board["streaming"];
};

const WORKING: ReadonlySet<Task["state"]> = new Set([
  "queued",
  "starting",
  "running",
  "blocked",
  "reviewing",
]);

const FINAL: ReadonlySet<Task["state"]> = new Set(["landed", "done", "rejected", "stopped", "failed"]);

/** Whether a task is over: its worker is gone and it will not run again. */
export function isFinal(task: Task): boolean {
  return FINAL.has(task.state);
}

/**
 * A worker whose card stays in view: it failed, or it waits for the user. A checker shows on
 * the row of what it checks, and an overnight run's held change waits for the run, not the user.
 */
function keepTask(task: Task): boolean {
  if (task.gateLink !== null) return false;
  return (
    task.state === "failed" ||
    task.state === "paused" ||
    task.state === "awaitingApproval" ||
    (task.state === "readyToLand" && task.run === null)
  );
}

/** Steps that belong to a worker's row (accepting, reading or messaging it), not rows of their own. */
const ON_TASK_ROW: ReadonlySet<OrchestratorStepKind["type"]> = new Set(["accepted", "readReport", "messaged"]);

/** Whether a decision is a judgement call the thread shows, rather than a task's or a plan's routine outcome. */
export function judgementCall(decision: Decision): boolean {
  return decision.source.type === "orchestrator" || decision.source.type === "run";
}

/** An overnight phase's own request: its lead's turns, tasks and reports. */
export function isRunRequest(requestId: string | null): boolean {
  return requestId?.startsWith("run-") ?? false;
}

function keepApproval(approval: Approval): boolean {
  switch (approval.state.type) {
    case "pending":
      return true;
    case "allowed":
    case "denied":
      return approval.state.by === "user";
    case "expired":
      return false;
  }
}

function keepPlan(plan: Plan): boolean {
  switch (plan.state.type) {
    case "proposed":
    case "inReview":
    case "revising":
    case "rejected":
      return true;
    case "approved":
      return plan.state.by === "user";
    case "superseded":
      return false;
  }
}

type Placed =
  | { kind: "message"; position: number; message: Message; text: string }
  | { kind: "card"; position: number; requestId: string | null; card: BlockCard }
  | { kind: "task"; position: number; requestId: string | null; id: string }
  | { kind: "row"; position: number; requestId: string | null; row: BlockRow; atMs: number }
  | {
      kind: "orchestrator";
      position: number;
      requestId: string | null;
      step: BlockOrchestratorStep;
      atMs: number;
    }
  | {
      kind: "compaction";
      position: number;
      requestId: string | null;
      after: string | null;
      compaction: BlockCompaction;
      atMs: number;
    };

function createdAt(board: BoardDigest, item: Exclude<Placed, { kind: "message" }>): number {
  if (item.kind === "task") return board.tasks[item.id]?.createdAtMs ?? 0;
  if (item.kind === "row" || item.kind === "orchestrator" || item.kind === "compaction") return item.atMs;
  const { type, id } = item.card;
  const card =
    type === "task"
      ? board.tasks[id]
      : type === "approval"
        ? board.approvals[id]
        : type === "question"
          ? board.questions[id]
          : board.plans[id];
  return card?.createdAtMs ?? 0;
}

/**
 * Groups the messages of a branch, the board's cards and the pending sends into blocks, oldest
 * first. Items of a request whose user message is on an older, unloaded page wait for it;
 * items of a request on another branch are not shown.
 */
export function buildBlocks(
  messages: readonly Message[],
  fullText: Readonly<Record<string, string>>,
  hasMore: boolean,
  board: BoardDigest,
  pending: readonly PendingMessage[],
): Block[] {
  const placed: Placed[] = messages.map((message) => ({
    kind: "message",
    position: message.seq,
    message,
    text: fullText[message.id] ?? message.text,
  }));
  // A whole phase's checks share one row, where the first of them started.
  const phaseChecks = new Map<string, Task>();
  for (const task of Object.values(board.tasks)) {
    placed.push({ kind: "task", position: task.position, requestId: task.requestId, id: task.id });
    // Each worker is one row; one that failed or waits for the user shows its card too.
    if (keepTask(task)) {
      placed.push({
        kind: "card",
        position: task.position,
        requestId: task.requestId,
        card: { type: "task", id: task.id, position: task.position, keep: true },
      });
    }
    const owner = task.gateLink?.owner;
    if (!owner) {
      placed.push({
        kind: "row",
        position: task.position,
        requestId: task.requestId,
        row: { type: "task", taskId: task.id, position: task.position },
        atMs: task.createdAtMs,
      });
    } else if (owner.type === "phase") {
      const key = `${owner.runId}:${owner.phaseId}`;
      const first = phaseChecks.get(key);
      if (!first || task.position < first.position) phaseChecks.set(key, task);
    }
  }
  for (const task of phaseChecks.values()) {
    if (task.gateLink?.owner.type !== "phase") continue;
    const { runId, phaseId } = task.gateLink.owner;
    placed.push({
      kind: "row",
      position: task.position,
      requestId: task.requestId,
      row: { type: "phaseChecks", runId, phaseId, position: task.position },
      atMs: task.createdAtMs,
    });
  }
  for (const step of board.orchestratorSteps) {
    // A phase's lead reads and messages its workers all night: the rows say what came of it.
    if (ON_TASK_ROW.has(step.kind.type) || isRunRequest(step.requestId)) continue;
    placed.push({
      kind: "orchestrator",
      position: step.position,
      requestId: step.requestId,
      step: { kind: step.kind, position: step.position },
      atMs: step.atMs,
    });
  }
  for (const decision of board.decisions) {
    if (!judgementCall(decision)) continue;
    placed.push({
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
    placed.push({
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
  for (const approval of Object.values(board.approvals)) {
    placed.push({
      kind: "card",
      position: approval.position,
      requestId: approval.requestId,
      card: { type: "approval", id: approval.id, position: approval.position, keep: keepApproval(approval) },
    });
  }
  for (const question of Object.values(board.questions)) {
    placed.push({
      kind: "card",
      position: question.position,
      requestId: question.requestId,
      card: { type: "question", id: question.id, position: question.position, keep: true },
    });
  }
  for (const plan of Object.values(board.plans)) {
    // A revision replaces the plan it revises in the thread; its card keeps the history.
    if (plan.state.type === "superseded") continue;
    placed.push({
      kind: "card",
      position: plan.position,
      requestId: plan.requestId,
      card: { type: "plan", id: plan.id, position: plan.position, keep: keepPlan(plan) },
    });
  }
  placed.sort((a, b) => a.position - b.position);
  // Work from before a request was answered again belongs to its earlier attempt.
  const current = placed.filter(
    (item) =>
      item.kind === "message" ||
      item.requestId === null ||
      createdAt(board, item) >= (board.requests[item.requestId]?.startedAtMs ?? 0),
  );

  const oldest = messages[0]?.seq ?? Number.POSITIVE_INFINITY;
  const blocks = new Map<string, Block>();
  const order: string[] = [];
  const open = (key: string, startedAtMs: number): Block => {
    let block = blocks.get(key);
    if (!block) {
      const request = board.requests[key];
      block = {
        key,
        user: null,
        texts: [],
        cards: [],
        tasks: [],
        rows: [],
        orchestratorSteps: [],
        steers: [],
        compactions: [],
        requestIds: [key],
        state: request?.state.type ?? "done",
        error: request?.state.type === "failed" ? request.state.error : null,
        startedAtMs: request?.startedAtMs ?? startedAtMs,
        endedAtMs: request?.endedAtMs ?? null,
      };
      blocks.set(key, block);
      order.push(key);
    }
    return block;
  };

  for (const request of Object.values(board.requests)) {
    if (request.id.startsWith("run-") && !request.id.endsWith("-report"))
      open(request.id, request.startedAtMs);
  }

  // Items from before requests existed belong to the user message before them.
  let latest: string | null = null;
  // The block each message of the branch shows in.
  const blockOf = new Map<string, string>();
  for (const item of current) {
    // Anything older than the loaded page waits until that page loads.
    if (hasMore && item.position < oldest) continue;
    if (item.kind === "message") {
      const { message } = item;
      if (message.role === "user") {
        const key = message.requestId ?? message.id;
        latest = key;
        blockOf.set(message.id, key);
        open(key, message.createdAtMs).user = { kind: "message", message, text: item.text };
        continue;
      }
      if (message.role === "system") continue;
      const key = message.requestId ?? latest ?? `orphan:${message.id}`;
      // A request whose message is on an older page, or on a branch not shown.
      if (message.requestId && !blocks.has(key) &&
        !(key.startsWith("run-") && board.requests[key])) continue;
      blockOf.set(message.id, key);
      open(key, message.createdAtMs).texts.push({
        messageId: message.id,
        position: item.position,
        text: item.text,
        model: message.model,
      });
      continue;
    }
    // A compaction between turns follows the answer it came after, on that branch only.
    const key =
      item.kind === "compaction" && item.requestId === null && item.after !== null
        ? (blockOf.get(item.after) ?? null)
        : (item.requestId ?? latest);
    if (key === null) continue;
    if (item.requestId && !blocks.has(key)) continue;
    const block = open(key, 0);
    if (item.kind === "task") block.tasks.push(item.id);
    else if (item.kind === "row") block.rows.push(item.row);
    else if (item.kind === "orchestrator") block.orchestratorSteps.push(item.step);
    else if (item.kind === "compaction") block.compactions.push(item.compaction);
    else block.cards.push(item.card);
  }

  // What streams belongs to the turn's request.
  const streaming = board.streaming;
  if (streaming && !messages.some((message) => message.id === streaming.messageId)) {
    const key = streaming.requestId ?? latest;
    const block = key !== null ? blocks.get(key) : undefined;
    block?.texts.push({
      messageId: streaming.messageId,
      position: Number.POSITIVE_INFINITY,
      text: streaming.text,
      model: null,
    });
  }

  // Synthetic phase requests have no user anchor. Interleave them by their recorded start,
  // so preparing them before the event walk cannot put a phase above the user's brief.
  const chronological = order.some((key) => key.startsWith("run-"))
    ? order.toSorted((a, b) => {
        const first = blocks.get(a)!;
        const second = blocks.get(b)!;
        return blockTime(first) - blockTime(second);
      })
    : order;
  const result = joinSteered(
    chronological.map((key) => blocks.get(key) as Block),
    board.requests,
  );
  // Sent but not yet stored: the bubble, and a block that works on it.
  for (const entry of pending) {
    result.push({
      key: entry.localId,
      user: { kind: "pending", pending: entry },
      texts: [],
      cards: [],
      tasks: [],
      rows: [],
      orchestratorSteps: [],
      steers: [],
      compactions: [],
      requestIds: [entry.localId],
      state: "working",
      error: null,
      startedAtMs: entry.createdAtMs,
      endedAtMs: null,
    });
  }
  return result;
}

function blockTime(block: Block): number {
  return block.user?.kind === "message" ? block.user.message.createdAtMs : block.startedAtMs;
}

/**
 * A request steered into the running turn of the block just before it joins that block: its
 * message becomes a bubble inside the block, its work follows, and the header times from the
 * steer.
 */
function joinSteered(blocks: Block[], requests: BoardDigest["requests"]): Block[] {
  const joined: Block[] = [];
  for (const block of blocks) {
    const into = requests[block.key]?.steeredInto;
    const previous = joined.at(-1);
    if (!into || !previous?.requestIds.includes(into) || block.user?.kind !== "message") {
      joined.push(block);
      continue;
    }
    const texts = [...previous.texts, ...block.texts];
    // The bubble shows after the reply that was streaming when it was sent.
    const after = texts.find((text) => text.messageId === requests[block.key]?.steeredAfter);
    const live = [previous.state, block.state].filter(isLive);
    const state = live.length > 0 ? (live.includes("working") ? "working" : "waiting") : block.state;
    joined[joined.length - 1] = {
      ...previous,
      texts: texts.toSorted((a, b) => a.position - b.position),
      cards: [...previous.cards, ...block.cards],
      tasks: [...previous.tasks, ...block.tasks],
      rows: [...previous.rows, ...block.rows],
      orchestratorSteps: [...previous.orchestratorSteps, ...block.orchestratorSteps],
      compactions: [...previous.compactions, ...block.compactions],
      steers: [
        ...previous.steers,
        {
          message: block.user.message,
          text: block.user.text,
          position: Math.max(block.user.message.seq, after ? after.position + 0.5 : 0),
        },
        ...block.steers,
      ],
      requestIds: [...previous.requestIds, ...block.requestIds],
      state,
      error: state === block.state ? block.error : null,
      startedAtMs: block.startedAtMs,
      endedAtMs:
        live.length > 0 ? null : Math.max(previous.endedAtMs ?? 0, block.endedAtMs ?? 0) || null,
    };
  }
  return joined;
}

/** What a block's work is made of, as its message metadata carries it. */
export type SequenceSource = {
  texts: readonly { position: number }[];
  cards: readonly BlockCard[];
  steers: readonly { position: number; text: string; atMs: number }[];
  compactions: readonly BlockCompaction[];
  orchestratorSteps: readonly BlockOrchestratorStep[];
  rows: readonly BlockRow[];
};

/** One line or card of a block's work. */
export type SequenceEntry =
  | { kind: "text"; index: number; position: number }
  | { kind: "card"; card: BlockCard; position: number }
  | { kind: "steer"; text: string; atMs: number; position: number }
  | { kind: "orchestrator"; steps: BlockOrchestratorStep[]; position: number }
  | { kind: "compaction"; compaction: BlockCompaction; position: number }
  | { kind: "row"; row: BlockRow; position: number };

/**
 * The block's replies, cards, rows and orchestrator steps in order. Adjacent orchestrator
 * steps share one line that opens to each, but a decision always stands on its own line.
 */
export function blockSequence(source: SequenceSource): SequenceEntry[] {
  const entries: SequenceEntry[] = [
    ...source.texts.map((text, index) => ({ kind: "text" as const, index, position: text.position })),
    ...source.cards.map((card) => ({ kind: "card" as const, card, position: card.position })),
    ...source.steers.map((steer) => ({ kind: "steer" as const, ...steer })),
    ...source.compactions
      .filter((compaction) => compaction.inTurn)
      .map((compaction) => ({ kind: "compaction" as const, compaction, position: compaction.position })),
    ...source.orchestratorSteps.map((step) => ({
      kind: "orchestrator" as const,
      steps: [step],
      position: step.position,
    })),
    ...source.rows.map((row) => ({ kind: "row" as const, row, position: row.position })),
  ].toSorted((a, b) => a.position - b.position);
  const decided = (entry: SequenceEntry) =>
    entry.kind === "orchestrator" && entry.steps.some((step) => step.kind.type === "decided");
  const merged: SequenceEntry[] = [];
  for (const entry of entries) {
    const previous = merged.at(-1);
    if (entry.kind === "orchestrator" && previous?.kind === "orchestrator" && !decided(entry) && !decided(previous)) {
      previous.steps.push(...entry.steps);
    } else merged.push(entry);
  }
  return merged;
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
    block.compactions.length > 0 ||
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

/**
 * The parent of `messages[at]` on its branch: its own, or (for messages from before branches
 * existed) the message before it. `null` at the start of the conversation; `undefined` when
 * the message before it is on an older page.
 */
function parentOf(messages: readonly Message[], at: number, hasMore: boolean): string | null | undefined {
  const { parentId } = messages[at] as Message;
  if (parentId !== null) return parentId === "" ? null : parentId;
  if (at > 0) return (messages[at - 1] as Message).id;
  return hasMore ? undefined : null;
}

/**
 * A node's sort order, never before its parent's: a message sent while the request before it
 * had not answered yet (queued or steered in) is older than that answer. The sort is stable
 * and parents are pushed first, so a tie keeps the parent ahead.
 */
function under(parent: { order: number } | null, order: number): number {
  return parent ? Math.max(order, parent.order) : order;
}

/**
 * The thread as assistant-ui's message tree. The branch that ends at `head` becomes blocks
 * with everything its requests did; with `branches` (a Chat), each message the user or the
 * model replaced on that branch comes along with everything that followed it, so the branch picker
 * can move between them.
 */
export function buildThread(
  messages: readonly Message[],
  fullText: Readonly<Record<string, string>>,
  hasMore: boolean,
  board: BoardDigest & { head: string | null },
  pending: readonly PendingMessage[],
  /**
   * Which other versions the tree carries for the branch picker: a Chat's edits and other
   * answers (`all`), or only a session's edits (`edits`), whose answers keep their work.
   */
  branches: "all" | "edits",
): ThreadTree {
  const index = new Map(messages.map((message, at) => [message.id, at]));
  const children = new Map<string | null, Message[]>();
  messages.forEach((message, at) => {
    const parent = parentOf(messages, at, hasMore);
    if (parent === undefined) return;
    const siblings = children.get(parent);
    if (siblings) siblings.push(message);
    else children.set(parent, [message]);
  });

  // The branch shown, back from its last message as far as the loaded pages go.
  const path: Message[] = [];
  let at = index.get(board.head ?? "") ?? (messages.length > 0 ? messages.length - 1 : undefined);
  while (at !== undefined) {
    path.push(messages[at] as Message);
    const parent = parentOf(messages, at, hasMore);
    const next = parent == null ? undefined : index.get(parent);
    at = next !== undefined && next < at ? next : undefined;
  }
  path.reverse();

  // Each node sorts by its oldest message, so siblings number oldest first ("1/2" is the
  // original) and every parent comes before its children.
  const nodes: { node: ThreadNode; order: number }[] = [];
  const nodeOf = new Map<string, { id: string; order: number }>();
  // A block keeps its id whichever branch is shown: a request's first answer is
  // `request:R`, a later one (answered again) is named after its first reply. A message sent
  // from here keeps the id it had while pending, and so does its block (see `shownIdOf`).
  const blockId = (block: Block, first: string | undefined): string => {
    const oldest = children.get(block.key)?.find((child) => child.role === "assistant")?.id;
    const key = shownIdOf(block.key);
    if (first === undefined) return oldest === undefined ? `request:${key}` : `request:${key}:next`;
    return oldest === undefined || first === oldest ? `request:${key}` : `request:${key}:${first}`;
  };
  const place = (
    blocks: readonly Block[],
    parent: { id: string; order: number } | null,
  ): { id: string; order: number } | null => {
    for (const block of blocks) {
      if (block.user) {
        const message = block.user.kind === "message" ? block.user.message : null;
        const id = message
          ? shownIdOf(message.id)
          : block.user.kind === "pending"
            ? block.user.pending.localId
            : block.key;
        const order = under(parent, message?.seq ?? Number.POSITIVE_INFINITY);
        nodes.push({ node: { id, parentId: parent?.id ?? null, kind: "user", block, head: message?.id ?? null }, order });
        if (message) nodeOf.set(message.id, { id, order });
        parent = { id, order };
      }
      if (!blockShows(block)) continue;
      const stored = block.texts.filter((text) => index.has(text.messageId));
      const id = blockId(block, stored[0]?.messageId);
      const order = under(parent, stored[0]?.position ?? (parent ? parent.order + 0.5 : 0));
      nodes.push({
        node: {
          id,
          parentId: parent?.id ?? null,
          kind: "block",
          block,
          head: stored.at(-1)?.messageId ?? (block.user?.kind === "message" ? block.user.message.id : null),
        },
        order,
      });
      for (const text of stored) nodeOf.set(text.messageId, { id, order });
      parent = { id, order };
    }
    return parent;
  };
  const headId = place(buildBlocks(path, fullText, hasMore, board, pending), null)?.id ?? null;

  const quiet: BoardDigest =
    branches === "all"
      ? { ...EMPTY_WORK, requests: board.requests }
      : { ...board, runRequest: null, streaming: null };
  // Every other version comes along with its whole subtree, not only its newest continuation:
  // a node that left the tree when another version was shown would come back last, and
  // assistant-ui would then number it after its younger siblings.
  const placed = new Set(path.map((message) => message.id));
  const branchOff = (line: readonly Message[]) => {
    for (const message of line) {
      if (branches === "edits" && message.role !== "user") continue;
      const parent = parentOf(messages, index.get(message.id) as number, hasMore);
      if (parent === undefined) continue;
      const parentNode = parent === null ? null : nodeOf.get(parent);
      if (parentNode === undefined) continue;
      const siblings = (children.get(parent) ?? []).filter(
        (other) => !placed.has(other.id) && other.role === message.role,
      );
      for (const sibling of siblings) {
        if (placed.has(sibling.id)) continue;
        // The replaced message and its newest continuation; older ones branch off it in turn.
        const chain = [sibling];
        for (let kids = children.get(sibling.id); kids?.length; kids = children.get(chain.at(-1)?.id ?? "")) {
          chain.push(kids.at(-1) as Message);
        }
        for (const link of chain) placed.add(link.id);
        // Another answer groups under its user message, which the tree already has.
        const user = sibling.role === "assistant" && parent !== null ? messages[index.get(parent) as number] : undefined;
        const blocks = buildBlocks(user ? [user, ...chain] : chain, fullText, false, quiet, []).map((block) =>
          settled(user && block.user?.kind === "message" && block.user.message.id === user.id ? { ...block, user: null } : block, chain),
        );
        place(blocks, parentNode);
        branchOff(chain);
      }
    }
  };
  branchOff(path);
  return { nodes: nodes.toSorted((a, b) => a.order - b.order).map(({ node }) => node), headId };
}

const EMPTY_WORK: BoardDigest = {
  tasks: {},
  approvals: {},
  questions: {},
  plans: {},
  requests: {},
  orchestratorSteps: [],
  decisions: [],
  compactions: {},
  runRequest: null,
  streaming: null,
};

/** A block on a branch the thread does not show: finished, timed by its own replies. */
function settled(block: Block, chain: readonly Message[]): Block {
  const times = chain
    .filter((message) => block.texts.some((text) => text.messageId === message.id))
    .map((message) => message.createdAtMs);
  const user = block.user?.kind === "message" ? block.user.message.createdAtMs : undefined;
  const start = user ?? times[0] ?? block.startedAtMs;
  return { ...block, state: "done", error: null, startedAtMs: start, endedAtMs: times.at(-1) ?? start };
}
