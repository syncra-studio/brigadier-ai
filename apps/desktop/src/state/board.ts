import { createContext, useContext } from "react";
import { createStore, type StoreApi, useStore } from "zustand";

import { showConversationNotice } from "@/state/notices";

import type {
  Approval,
  Compaction,
  ContextUsage,
  ConversationView,
  Decision,
  DiffStat,
  EventEnvelope,
  MachineStep,
  MemoryChange,
  MessageQueue,
  Notice,
  OrchestratorLogEntry,
  OrchestratorStep,
  OvernightRun,
  Plan,
  ProviderEvent,
  Question,
  RawEntry,
  RunState,
  StreamingMessage,
  ThinkingSegment,
  Task,
  UserRequest,
  Rating,
  RebirthThresholds,
  WaitingItem,
  WorkerStep,
} from "@/ipc/generated";

/** Newest entries kept per open worker transcript; older ones load on demand. */
export const WORKER_ENTRIES = 3_000;

/** Newest orchestrator log entries kept for the Inspector. */
export const ORCHESTRATOR_ENTRIES = 5_000;

/** Notices kept per conversation, newest last. */
const NOTICES = 20;

/** The loaded part of a worker's transcript, oldest first. */
export type WorkerTranscript = {
  entries: RawEntry[];
  hasMore: boolean;
  loading: boolean;
};

/**
 * The open conversation's live state: its tasks, cards, queue, run state and streaming text.
 * Only the open conversation has a board, so only its events cost anything.
 */
export type Board = {
  conversationId: string;
  loaded: boolean;
  tasks: Record<string, Task>;
  approvals: Record<string, Approval>;
  questions: Record<string, Question>;
  plans: Record<string, Plan>;
  /** Overnight runs, a segment each, by id. */
  overnight: Record<string, OvernightRun>;
  /** What each user message set in motion, by request id (the message's id). */
  requests: Record<string, UserRequest>;
  /** Every worker step (started, finished, …), in stream order. */
  workerSteps: WorkerStep[];
  /** Every orchestrator step (messaged a worker, read a report, …), in stream order. */
  orchestratorSteps: OrchestratorStep[];
  /** Every row about the machine (waiting for it to cool down, a build paused), in stream order. */
  machineSteps: MachineStep[];
  /** What was decided on the user's behalf, in stream order. */
  decisions: Decision[];
  /** What only the user can do and is not done yet, by id. */
  waiting: Record<string, WaitingItem>;
  /** A Chat's context compactions, by id. */
  compactions: Record<string, Compaction>;
  /** The user's ratings of answers, by subject (a message id, or `task:<id>`). */
  ratings: Partial<Record<string, Rating>>;
  queue: MessageQueue;
  run: RunState;
  runError: string | null;
  /** The request the running turn serves. */
  runRequest: string | null;
  /** The last message of the branch the thread shows. */
  head: string | null;
  /** What the orchestrator (or a Chat's model) is doing right now, in a few words. */
  doing: string | null;
  /** How full the conversation model's context is; absent until its CLI first said. */
  context: ContextUsage | null;
  streaming: StreamingMessage | null;
  thinking: ThinkingSegment[];
  notices: Notice[];
  /** What each worker is doing right now, in a few words (from its live events). */
  activity: Record<string, string>;
  /** Each worker's latest reply, its first line, and when it came (from its live events). */
  summaries: Record<string, WorkerSummary>;
  /** How many times each worker changed files or ran a command (from its live events). */
  edits: Record<string, number>;
  /** What each worker at work on a change changed in its worktree so far, as last read. */
  diffs: Record<string, DiffStat>;
  /** Transcripts of the worker cards opened so far, by task id. */
  transcripts: Record<string, WorkerTranscript>;
  /** A Chat's Memory chips: the latest change per memory, in the order they were saved. */
  memories: MemoryChange[];
};

/** A worker's latest reply, as its status line in the Workers panel. */
export type WorkerSummary = { text: string; atMs: number };

/** The Inspector's Orchestrator tab: one conversation's orchestrator log. */
export type OrchestratorLog = {
  conversationId: string;
  entries: OrchestratorLogEntry[];
  hasMore: boolean;
  loading: boolean;
  /** When the orchestrator is reborn, for its current model; read with the log. */
  thresholds: RebirthThresholds | null;
};

type BoardState = {
  board: Board | null;
  orchestrator: OrchestratorLog | null;
};

export type BoardStore = StoreApi<BoardState>;

/** The open conversation's board (and the Inspector's orchestrator log). */
const mainBoard: BoardStore = createStore<BoardState>()(() => ({
  board: null,
  orchestrator: null,
}));

/** Side chats' boards, each shown beside the open conversation in its side panel. */
const sideBoards = new Set<BoardStore>();

/** The board the components below read: the open conversation's, unless a side chat's. */
export const BoardStoreContext = createContext<BoardStore>(mainBoard);

/**
 * Reads the board of the view it renders in. `getState`/`setState` are the open
 * conversation's board, for code outside any view.
 */
export function useBoard<T>(selector: (state: BoardState) => T): T {
  return useStore(useContext(BoardStoreContext), selector);
}
useBoard.getState = mainBoard.getState;
useBoard.setState = mainBoard.setState;

/** Something about the board that changes the checkout: a landing, an Undo or Reapply. */
export function useCheckoutChanges(conversationId: string): string {
  return useBoard((s) => {
    const board = s.board;
    if (!board || board.conversationId !== conversationId) return "";
    const landed = Object.values(board.tasks)
      .map((task) => task.landed ?? "")
      .join();
    const undone = Object.values(board.requests)
      .map((entry) => entry.undo?.commits.at(-1) ?? "")
      .join();
    return `${landed}|${undone}`;
  });
}

/** A board for a side chat, fed by the same events as the open one's until disposed. */
export function createSideBoard(conversationId: string): BoardStore {
  const store = createStore<BoardState>()(() => ({
    board: emptyBoard(conversationId),
    orchestrator: null,
  }));
  sideBoards.add(store);
  return store;
}

export function disposeSideBoard(store: BoardStore): void {
  sideBoards.delete(store);
}

/** The board showing `conversationId`, the open one's or a side chat's, if any does. */
/**
 * The newest request, while the answer the thread shows for it still works or waits (it, or a
 * request it was steered into, has a turn, a worker or a card going), as the daemon judges it:
 * what a session's user sends now waits in the queue while the orchestrator sorts it.
 */
export function workingRequest(board: Board): string | null {
  const requests = Object.values(board.requests);
  const latest = requests.reduce<UserRequest | null>(
    (newest, request) =>
      !newest ||
      request.startedAtMs > newest.startedAtMs ||
      (request.startedAtMs === newest.startedAtMs && request.id > newest.id)
        ? request
        : newest,
    null,
  );
  let request = latest;
  // A steer chain is short; the bound only guards against a cycle in stored data.
  for (let hops = 0; request && hops < requests.length; hops += 1) {
    if (request.state.type === "working" || request.state.type === "waiting") return latest?.id ?? null;
    request = request.steeredInto ? (board.requests[request.steeredInto] ?? null) : null;
  }
  return null;
}

export function boardOf(conversationId: string): Board | null {
  for (const store of [mainBoard, ...sideBoards]) {
    const { board } = store.getState();
    if (board?.conversationId === conversationId) return board;
  }
  return null;
}

/** The conversations side boards show. */
export function sideBoardIds(): string[] {
  return [...sideBoards].flatMap((store) => store.getState().board?.conversationId ?? []);
}


const EMPTY_QUEUE: MessageQueue = { items: [], paused: false };

export function emptyBoard(conversationId: string): Board {
  return {
    conversationId,
    loaded: false,
    tasks: {},
    approvals: {},
    questions: {},
    plans: {},
    overnight: {},
    requests: {},
    workerSteps: [],
    orchestratorSteps: [],
    machineSteps: [],
    decisions: [],
    waiting: {},
    compactions: {},
    ratings: {},
    queue: EMPTY_QUEUE,
    run: "idle",
    runError: null,
    runRequest: null,
    head: null,
    doing: null,
    context: null,
    streaming: null,
    thinking: [],
    notices: [],
    activity: {},
    summaries: {},
    edits: {},
    diffs: {},
    transcripts: {},
    memories: [],
  };
}

function byId<T extends { id: string }>(items: readonly T[]): Record<string, T> {
  return Object.fromEntries(items.map((item) => [item.id, item]));
}

/**
 * Board events that replace an object outright, so applying one again is harmless. A view is
 * read at an unknown point in the event stream, so these are re-applied on top of it.
 */
const REPLAYED = new Set<EventEnvelope["event"]["type"]>([
  "taskUpdated",
  "approvalUpdated",
  "questionUpdated",
  "planUpdated",
  "overnightUpdated",
  "queueChanged",
  "runStateChanged",
  "requestUpdated",
  "messageAppended",
  "branchSwitched",
  "workerStepped",
  "orchestratorStepped",
  "machineStepped",
  "thinkingDelta",
  "compactionUpdated",
  "messageRated",
  "memoryUpdated",
  "decidedForYou",
  "waitingOnYou",
  "waitingResolved",
]);

/** Conversation view reads in flight, each collecting the board events that arrive meanwhile. */
const viewLoads = new Set<{ conversationId: string; events: EventEnvelope[] }>();

/**
 * Starts collecting a conversation's board events while its view is read. The returned
 * function stops collecting and returns them, to pass to `boardFromView`.
 */
export function collectBoardEvents(conversationId: string): () => EventEnvelope[] {
  const load = { conversationId, events: [] as EventEnvelope[] };
  viewLoads.add(load);
  return () => {
    viewLoads.delete(load);
    return load.events;
  };
}

/**
 * Replaces the board's snapshot with a fresh read, keeping opened transcripts. Events that
 * arrived while it was read are applied again, since the read may predate them.
 */
export function boardFromView(
  view: ConversationView,
  previous: Board | null,
  arrived: readonly EventEnvelope[],
): Board {
  const keep = previous?.conversationId === view.conversation.id ? previous : null;
  const board: Board = {
    conversationId: view.conversation.id,
    loaded: true,
    tasks: byId(view.tasks),
    approvals: byId(view.approvals),
    questions: byId(view.questions),
    plans: byId(view.plans),
    overnight: byId(view.overnight),
    requests: byId(view.requests),
    workerSteps: view.workerSteps,
    orchestratorSteps: view.orchestratorSteps,
    machineSteps: view.machineSteps ?? [],
    decisions: view.decisions,
    waiting: byId(view.waiting),
    compactions: byId(view.compactions),
    ratings: view.ratings,
    queue: view.queue,
    run: view.run,
    runError: keep?.runError ?? null,
    runRequest: view.runRequest,
    head: view.head,
    doing: keep?.doing ?? null,
    context: view.context,
    streaming: view.streaming,
    thinking: view.thinking ?? [],
    notices: view.notices.filter(showConversationNotice).slice(-NOTICES),
    activity: keep?.activity ?? {},
    summaries: keep?.summaries ?? {},
    edits: keep?.edits ?? {},
    diffs: keep?.diffs ?? {},
    transcripts: keep?.transcripts ?? {},
    memories: view.memories,
  };
  return arrived.reduce(applyToBoard, board);
}

/** A few words on what a worker event shows it doing, or `undefined` to keep the last one. */
function activityOf(event: ProviderEvent): string | null | undefined {
  switch (event.type) {
    case "turnStarted":
      return "Working…";
    case "reasoningDelta":
    case "reasoning":
      return "Thinking…";
    case "messageDelta":
      return "Writing…";
    case "message":
      return event.role === "assistant" ? firstLine(event.text) : undefined;
    case "command":
      return event.command ? `$ ${firstLine(event.command)}` : undefined;
    case "toolCall":
      return event.name;
    case "fileChanges":
      return `Editing ${event.changes.length} file${event.changes.length === 1 ? "" : "s"}`;
    case "approvalRequested":
      return "Waiting for approval";
    case "error":
      return event.error.willRetry ? `Retrying: ${event.error.kind}` : `Error: ${event.error.kind}`;
    case "turnCompleted":
    case "exited":
      return null;
    default:
      return undefined;
  }
}

function firstLine(text: string): string {
  const line = text.trimStart().split("\n", 1)[0] ?? "";
  return line.length > 120 ? `${line.slice(0, 120)}…` : line;
}

function appendEntry(transcript: WorkerTranscript, entry: RawEntry): WorkerTranscript {
  const last = transcript.entries.at(-1);
  if (last && last.streamSeq >= entry.streamSeq) return transcript;
  const entries = [...transcript.entries, entry];
  // Trim in steps, so a long transcript is not re-sliced (and re-folded) on every event.
  const trimmed = entries.length > WORKER_ENTRIES + WORKER_ENTRIES / 5;
  return {
    ...transcript,
    entries: trimmed ? entries.slice(-WORKER_ENTRIES) : entries,
    hasMore: transcript.hasMore || trimmed,
  };
}

/**
 * Stores an updated task or card at its place in the thread. As in the daemon's board, an
 * object's position is the conversation stream sequence of the event that first recorded it;
 * the event itself doesn't carry it.
 */
function placed<T extends { id: string; position: number }>(
  items: Record<string, T>,
  item: T,
  envelope: EventEnvelope,
  board: Board,
): Record<string, T> {
  const known = items[item.id];
  const position =
    known?.position ??
    (envelope.stream === `conversation:${board.conversationId}` ? envelope.streamSeq : item.position);
  return { ...items, [item.id]: { ...item, position } };
}

export function applyToBoard(board: Board, envelope: EventEnvelope): Board {
  const { event, streamSeq, atMs } = envelope;
  switch (event.type) {
    case "messageAppended":
      // A new message continues the branch shown; the final message replaces the text that
      // streamed for it.
      return {
        ...board,
        head: event.message.id,
        streaming: board.streaming?.messageId === event.message.id ? null : board.streaming,
      };
    case "branchSwitched":
      return { ...board, head: event.head };
    case "thinkingDelta": {
      const known = board.thinking.find((segment) => segment.itemId === event.itemId);
      if (known && streamSeq <= known.throughPosition) return board;
      const segment: ThinkingSegment = known
        ? { ...known, text: event.complete ? event.text : known.complete ? known.text : known.text + event.text,
            updatedAtMs: event.atMs, throughPosition: streamSeq, complete: known.complete || event.complete }
        : { itemId: event.itemId, requestId: event.requestId, text: event.text, position: streamSeq,
            startedAtMs: event.atMs, updatedAtMs: event.atMs, throughPosition: streamSeq, complete: event.complete };
      return { ...board, thinking: known
        ? board.thinking.map((item) => item === known ? segment : item)
        : [...board.thinking, segment] };
    }
    case "messageDelta": {
      const current = board.streaming;
      const streaming =
        current?.messageId === event.messageId
          ? { ...current, text: current.text + event.text }
          : { messageId: event.messageId, text: event.text, requestId: board.runRequest };
      return { ...board, streaming };
    }
    case "orchestratorLogged": {
      if (event.entry.type !== "provider") return board;
      const provided = event.entry.event;
      if (provided.type === "contextSize") {
        return {
          ...board,
          context: {
            usedTokens: provided.usedTokens,
            windowTokens: provided.windowTokens ?? board.context?.windowTokens ?? null,
          },
        };
      }
      const doing = doingOf(provided, board.doing);
      return doing === board.doing ? board : { ...board, doing };
    }
    case "runStateChanged":
      return {
        ...board,
        doing: event.state === "running" || event.state === "starting" ? board.doing : null,
        run: event.state,
        runError: event.error,
        runRequest: event.requestId,
        // A turn that ended without a final message leaves nothing streaming.
        streaming: event.state === "running" || event.state === "starting" ? board.streaming : null,
      };
    case "conversationNotice":
      if (!showConversationNotice(event.notice)) return board;
      return { ...board, notices: [...board.notices, event.notice].slice(-NOTICES) };
    case "taskUpdated":
      return { ...board, tasks: placed(board.tasks, event.task, envelope, board) };
    case "approvalUpdated":
      return { ...board, approvals: placed(board.approvals, event.approval, envelope, board) };
    case "questionUpdated":
      return { ...board, questions: placed(board.questions, event.question, envelope, board) };
    case "planUpdated":
      return { ...board, plans: placed(board.plans, event.plan, envelope, board) };
    case "overnightUpdated":
      return { ...board, overnight: { ...board.overnight, [event.run.id]: event.run } };
    case "requestUpdated":
      return { ...board, requests: { ...board.requests, [event.request.id]: event.request } };
    case "workerStepped":
      // Applied again after a view read: a step is kept once.
      return board.workerSteps.some((step) => step.position === streamSeq)
        ? board
        : { ...board, workerSteps: [...board.workerSteps, { ...event.step, position: streamSeq }] };
    case "orchestratorStepped": {
      const kind = event.step.kind;
      const known = kind.type === "tool"
        ? board.orchestratorSteps.find((step) => step.kind.type === "tool" && step.kind.itemId === kind.itemId)
        : board.orchestratorSteps.find((step) => step.position === streamSeq);
      if (known && (known.kind.type !== "tool" || streamSeq <= known.kind.throughPosition)) return board;
      const step = known
        ? { ...known, kind: { ...kind, throughPosition: streamSeq } }
        : { ...event.step, position: streamSeq, kind: kind.type === "tool" ? { ...kind, throughPosition: streamSeq } : kind };
      return { ...board, orchestratorSteps: known
        ? board.orchestratorSteps.map((item) => item === known ? step : item)
        : [...board.orchestratorSteps, step] };
    }
    case "machineStepped":
      return board.machineSteps.some((step) => step.position === streamSeq)
        ? board
        : { ...board, machineSteps: [...board.machineSteps, { ...event.step, position: streamSeq }] };
    case "decidedForYou":
      // Applied again after a view read: a decision is kept once.
      return board.decisions.some((decision) => decision.id === event.decision.id)
        ? board
        : { ...board, decisions: [...board.decisions, { ...event.decision, position: streamSeq }] };
    case "waitingOnYou":
      return { ...board, waiting: { ...board.waiting, [event.item.id]: event.item } };
    case "waitingResolved": {
      if (!board.waiting[event.id]) return board;
      const { [event.id]: _done, ...waiting } = board.waiting;
      return { ...board, waiting };
    }
    case "compactionUpdated":
      return { ...board, compactions: placed(board.compactions, event.compaction, envelope, board) };
    case "messageRated":
      return { ...board, ratings: { ...board.ratings, [event.subject]: event.rating } };
    case "queueChanged":
      return { ...board, queue: event.queue };
    case "memoryUpdated": {
      // A change to a memory already shown (the user removed it) updates it in place.
      const { memory } = event;
      const known = board.memories.some((entry) => entry.nodeId === memory.nodeId);
      return {
        ...board,
        memories: known
          ? board.memories.map((entry) => (entry.nodeId === memory.nodeId ? memory : entry))
          : [...board.memories, memory],
      };
    }
    case "workerEvent": {
      const { taskId } = event;
      let next = board;
      const activity = activityOf(event.event);
      if (activity !== undefined && (board.activity[taskId] ?? null) !== activity) {
        const { [taskId]: _previous, ...rest } = board.activity;
        next = {
          ...next,
          activity: activity === null ? rest : { ...rest, [taskId]: activity },
        };
      }
      const reply = event.event;
      if (reply.type === "message" && reply.role === "assistant") {
        const text = firstLine(reply.text);
        if (text) next = { ...next, summaries: { ...next.summaries, [taskId]: { text, atMs } } };
      } else if (
        (reply.type === "fileChanges" || reply.type === "command") &&
        reply.status !== "inProgress"
      ) {
        // A command may have changed files too.
        next = { ...next, edits: { ...next.edits, [taskId]: (next.edits[taskId] ?? 0) + 1 } };
      }
      const transcript = board.transcripts[taskId];
      if (transcript) {
        const updated = appendEntry(transcript, { streamSeq, atMs, event: event.event });
        if (updated !== transcript) {
          next = { ...next, transcripts: { ...next.transcripts, [taskId]: updated } };
        }
      }
      return next;
    }
    default:
      return board;
  }
}

/** What a Brigadier tool call or CLI tool of the orchestrator is doing, by tool name. */
const TOOL_DOING: Readonly<Record<string, string>> = {
  delegate_task: "Delegating to a worker",
  message_worker: "Messaging a worker",
  answer_worker: "Answering a worker",
  stop_worker: "Stopping a worker",
  ask_user: "Asking you",
  read_report: "Reading a report",
  read_artifact: "Reading a report",
  query_brain: "Checking the project notes",
  search_transcript: "Looking back through the conversation",
  // Remembering is silent by design.
  remember: "Thinking",
  plan_phases: "Planning the phases",
  approve_outline: "Reading a worker's outline",
  request_approval: "Asking for your approval",
  land_phase: "Landing a worker's commits",
  finish_session: "Finishing the session",
  list_tasks: "Checking on the workers",
  route_follow_up: "Sorting your follow-up",
  note_for_user: "Noting it for you",
  WebSearch: "Searching the web",
  WebFetch: "Reading a web page",
};

/** The orchestrator's current activity after one of its provider events. */
function doingOf(event: ProviderEvent, current: string | null): string | null {
  switch (event.type) {
    case "toolCall": {
      if (event.status !== "inProgress") return null;
      // MCP tools arrive namespaced (`mcp__brigadier__delegate_task`, `brigadier.delegate_task`).
      const name = event.name.split(/__|\./).at(-1) ?? event.name;
      return TOOL_DOING[name] ?? "Using a tool";
    }
    case "reasoning":
    case "message":
    case "turnCompleted":
    case "exited":
      return null;
    default:
      return current;
  }
}

function applyToLog(log: OrchestratorLog, envelope: EventEnvelope): OrchestratorLog {
  const { event, streamSeq, atMs } = envelope;
  if (event.type !== "orchestratorLogged") return log;
  const last = log.entries.at(-1);
  if (last && last.streamSeq >= streamSeq) return log;
  const entries = [...log.entries, { streamSeq, atMs, entry: event.entry }];
  const trimmed = entries.length > ORCHESTRATOR_ENTRIES;
  return {
    ...log,
    entries: trimmed ? entries.slice(-ORCHESTRATOR_ENTRIES) : entries,
    hasMore: log.hasMore || trimmed,
  };
}

/**
 * Routes a batch of events to the open conversation's board, the side chats' boards and the
 * Inspector's orchestrator log, one update each. Everything else is ignored here: other
 * conversations have no board.
 */
export function applyBoardEvents(envelopes: readonly EventEnvelope[]): void {
  for (const load of viewLoads) {
    const stream = `conversation:${load.conversationId}`;
    for (const envelope of envelopes) {
      if (envelope.stream === stream && REPLAYED.has(envelope.event.type)) {
        load.events.push(envelope);
      }
    }
  }
  for (const store of [mainBoard, ...sideBoards]) applyToStore(store, envelopes);
}

function applyToStore(store: BoardStore, envelopes: readonly EventEnvelope[]): void {
  const { board, orchestrator } = store.getState();
  if (!board && !orchestrator) return;
  const conversationStream = board ? `conversation:${board.conversationId}` : null;
  const boardLogStream = board ? `orch:${board.conversationId}` : null;
  const orchestratorStream = orchestrator ? `orch:${orchestrator.conversationId}` : null;
  let nextBoard = board;
  let nextLog = orchestrator;
  for (const envelope of envelopes) {
    const { stream } = envelope;
    // The open conversation's own log says what its model is doing right now.
    if (nextBoard && stream === boardLogStream) nextBoard = applyToBoard(nextBoard, envelope);
    if (nextBoard && stream === conversationStream) {
      nextBoard = applyToBoard(nextBoard, envelope);
    } else if (
      nextBoard &&
      stream.startsWith("task:") &&
      nextBoard.tasks[stream.slice("task:".length)]
    ) {
      nextBoard = applyToBoard(nextBoard, envelope);
    } else if (nextLog && stream === orchestratorStream) {
      nextLog = applyToLog(nextLog, envelope);
    }
  }
  if (nextBoard !== board || nextLog !== orchestrator) {
    store.setState({ board: nextBoard, orchestrator: nextLog });
  }
}

/** Updates every board that shows the given conversation (the open one's, a side chat's). */
export function updateBoard(conversationId: string, update: (board: Board) => Board): void {
  for (const store of [mainBoard, ...sideBoards]) {
    store.setState((state) =>
      state.board?.conversationId === conversationId ? { board: update(state.board) } : state,
    );
  }
}
