import { createContext, useContext } from "react";
import { createStore, type StoreApi, useStore } from "zustand";

import { liveActivity } from "@/app/conversation/activity/words";
import { previewActive } from "@/app/conversation/previewStatus";
import { showConversationNotice, shownNotice } from "@/state/notices";
import { type Stop, stopsOf, useStopping } from "@/state/stopping";

import type {
  AgentMcpServers,
  Approval,
  ApprovalSubject,
  Compaction,
  ContextUsage,
  ConversationView,
  Decision,
  DiffStat,
  EventEnvelope,
  MachineStep,
  MessageQueue,
  Notice,
  OrchestratorLogEntry,
  OrchestratorStep,
  PeerMessage,
  PeerState,
  Preview,
  ProviderEvent,
  Question,
  RawEntry,
  RunState,
  StreamingMessage,
  ThinkingSegment,
  Task,
  ThreadNews,
  UserRequest,
  Rating,
  RebirthThresholds,
  WorkerStep,
} from "@/ipc/generated";

/**
 * A card the thread shows: any approval but a merge card from a recorded conversation. The user
 * asks for a merge in words now; the thread merges when they do.
 */
export type ShownApproval = Approval & {
  subject: Exclude<ApprovalSubject, { type: "finishSession" }>;
};

export function shownApproval(approval: Approval): approval is ShownApproval {
  return approval.subject.type !== "finishSession";
}

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
  approvals: Record<string, ShownApproval>;
  questions: Record<string, Question>;
  /** The thread's previews (a dev server, the app it runs for the user), by id. */
  previews: Record<string, Preview>;
  /** The ended previews the user dismissed from the Background rows (Q94), by id. */
  backgroundCleared: string[];
  /** The MCP servers of each agent whose CLI runs, by `mcpAgentKey` (its own model's, or a worker's task id). */
  mcpServers: Record<string, AgentMcpServers>;
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
  /** The session branch's tip after the thread's last commit seen since the view was read: it moves with each of its commits. */
  threadTip: string | null;
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
  /** Its messages to and from other sessions, by message id (SESSION-MESSAGING-PLAN.md §6). */
  peerMessages: Record<string, PeerMessage>;
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

/** Boards kept of conversations shown before: the newest ones the user left. */
const KEPT_BOARDS = 8;

/**
 * The loaded boards of conversations no view shows now, oldest first, kept current by their
 * events: a conversation shown again shows its true state at once while its view is read again
 * (#484), never an empty board that looks finished.
 */
const keptBoards = new Map<string, Board>();

/** Keeps a board no view shows any more, unless it never loaded. */
export function keepBoard(board: Board | null): void {
  if (!board?.loaded) return;
  keptBoards.delete(board.conversationId);
  keptBoards.set(board.conversationId, board);
  for (const id of keptBoards.keys()) {
    if (keptBoards.size <= KEPT_BOARDS) break;
    keptBoards.delete(id);
  }
}

/** Takes a kept board out to show it, if one is kept. */
export function takeKeptBoard(conversationId: string): Board | null {
  const board = keptBoards.get(conversationId) ?? null;
  keptBoards.delete(conversationId);
  return board;
}

/** Whether a conversation's board is loaded, shown or kept: it can show at once. */
export function boardHeld(conversationId: string): boolean {
  return !!keptBoards.get(conversationId)?.loaded || !!boardOf(conversationId)?.loaded;
}

/** Forgets the kept boards: events may have been missed (a resync), so they may be stale. */
export function forgetKeptBoards(): void {
  keptBoards.clear();
}

/** The board the components below read: the open conversation's, unless a side chat's. */
export const BoardStoreContext = createContext<BoardStore>(mainBoard);

/** A worker that is over, as the board shows it. */
const WORKER_OVER: ReadonlySet<Task["state"]> = new Set(["reported", "landed", "done", "rejected", "stopped", "failed"]);

/** `board` as the user's stops under way show it: their workers stopped, a stopped turn over. */
export function stoppedBoard(board: Board, stops: readonly Stop[]): Board {
  let shown = board;
  for (const stop of stopsOf(stops, board.conversationId)) {
    let tasks = shown.tasks;
    for (const id of stop.tasks) {
      const task = tasks[id];
      if (!task || WORKER_OVER.has(task.state)) continue;
      tasks = { ...tasks, [id]: { ...task, state: "stopped", updatedAtMs: Math.max(task.updatedAtMs, stop.atMs) } };
    }
    if (tasks !== shown.tasks) shown = { ...shown, tasks };
    if (!stop.turn) continue;
    // The answers it worked on end with the turn, as the daemon ends them.
    const open = Object.values(shown.requests).filter((request) => request.state.type === "working");
    const running = shown.run === "running" || shown.run === "starting";
    if (!running && open.length === 0 && shown.doing === null) continue;
    shown = {
      ...shown,
      run: running ? "idle" : shown.run,
      doing: null,
      requests: open.length === 0
        ? shown.requests
        : { ...shown.requests, ...Object.fromEntries(open.map((request) => [request.id, { ...request, state: { type: "stopped" } as const }])) },
    };
  }
  return shown;
}

const shownStates = new WeakMap<BoardState, { stops: readonly Stop[]; state: BoardState }>();

/** `state` with its board as the stops under way show it; the same state when none touch it. */
function shownState(state: BoardState, stops: readonly Stop[]): BoardState {
  const { board } = state;
  if (!board || stopsOf(stops, board.conversationId).length === 0) return state;
  const cached = shownStates.get(state);
  if (cached?.stops === stops) return cached.state;
  const shown = { ...state, board: stoppedBoard(board, stops) };
  shownStates.set(state, { stops, state: shown });
  return shown;
}

/**
 * Reads the board of the view it renders in, as the user's stops under way show it (#498).
 * `getState`/`setState` are the open conversation's board as it really is, for code outside
 * any view.
 */
export function useBoard<T>(selector: (state: BoardState) => T): T {
  const stops = useStopping((s) => s.stops);
  return useStore(useContext(BoardStoreContext), (state) => selector(shownState(state, stops)));
}
useBoard.getState = mainBoard.getState;
useBoard.setState = mainBoard.setState;
useBoard.subscribe = mainBoard.subscribe;

/** Something about the board that changes the checkout: a landing, an Undo or Reapply, a review. */
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
    // A review's Undo puts lines back; its Keep moves what is left to review.
    const reviewed = Object.values(board.requests)
      .map((entry) => Object.entries(entry.review ?? {}).join())
      .join("|");
    return `${landed}|${undone}|${reviewed}`;
  });
}

/** A board for a side chat, fed by the same events as the open one's until disposed. */
export function createSideBoard(conversationId: string): BoardStore {
  const store = createStore<BoardState>()(() => ({
    board: takeKeptBoard(conversationId) ?? emptyBoard(conversationId),
    orchestrator: null,
  }));
  sideBoards.add(store);
  return store;
}

export function disposeSideBoard(store: BoardStore): void {
  sideBoards.delete(store);
  keepBoard(store.getState().board);
}

/** The newest request's id and the request of its steer chain that still works, if any. */
export function liveRequest(board: Pick<Board, "requests">): { newest: string; live: UserRequest } | null {
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
    if (latest && request.state.type === "working") return { newest: latest.id, live: request };
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
    previews: {},
    backgroundCleared: [],
    mcpServers: {},
    requests: {},
    workerSteps: [],
    orchestratorSteps: [],
    machineSteps: [],
    decisions: [],
    compactions: {},
    ratings: {},
    queue: EMPTY_QUEUE,
    run: "idle",
    runError: null,
    runRequest: null,
    head: null,
    threadTip: null,
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
    peerMessages: {},
  };
}

/** An agent's key in `Board.mcpServers`: its worker's task id, or `lead` for the conversation's own model. */
export function mcpAgentKey(taskId: string | null): string {
  return taskId === null ? "lead" : `task:${taskId}`;
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
  "mcpServersUpdated",
  "previewUpdated",
  "previewsCleared",
  "backgroundCleared",
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
  "ratingCleared",
  "decidedForYou",
  "peerMessageSent",
  "peerMessageReceived",
  "peerMessageState",
  "peerWaitSettled",
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
    approvals: byId(view.approvals.filter(shownApproval)),
    questions: byId(view.questions),
    // A daemon from before previews sends none.
    previews: byId(view.previews ?? []),
    // Nor one from before the merge rule.
    backgroundCleared: view.backgroundCleared ?? [],
    // Nor one from before agents told their MCP servers.
    mcpServers: Object.fromEntries((view.mcpServers ?? []).map((agent) => [mcpAgentKey(agent.taskId), agent])),
    requests: byId(view.requests),
    workerSteps: view.workerSteps,
    orchestratorSteps: view.orchestratorSteps,
    machineSteps: view.machineSteps ?? [],
    decisions: view.decisions,
    compactions: byId(view.compactions),
    ratings: view.ratings,
    queue: view.queue,
    run: view.run,
    runError: keep?.runError ?? null,
    runRequest: view.runRequest,
    head: view.head,
    threadTip: keep?.threadTip ?? null,
    doing: keep?.doing ?? null,
    context: view.context,
    streaming: view.streaming,
    thinking: view.thinking ?? [],
    notices: view.notices.filter(showConversationNotice).slice(-NOTICES).map(shownNotice),
    activity: keep?.activity ?? {},
    summaries: keep?.summaries ?? {},
    edits: keep?.edits ?? {},
    diffs: keep?.diffs ?? {},
    transcripts: keep?.transcripts ?? {},
    // A daemon from before messages between sessions sends none.
    peerMessages: Object.fromEntries((view.peerMessages ?? []).map((message) => [message.messageId, message])),
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
      return event.command ? firstLine(liveActivity({ command: event.command })) : undefined;
    case "toolCall":
      return liveActivity(event);
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

/**
 * A message's state after a change, as the daemon's board keeps it: delivery never goes back, so
 * a late "delivered" or "seen" after the receiver took or answered it changes nothing.
 */
export function peerStateAfter(current: PeerState, next: PeerState): PeerState {
  if (next.type === "delivered" && (current.type === "seen" || current.type === "replied")) return current;
  if (next.type === "seen" && current.type === "replied") return current;
  return next;
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
    case "threadCommitsSeen":
      return { ...board, threadTip: event.tip };
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
      const phase = current?.messageId === event.messageId ? (current.phase ?? event.phase) : event.phase;
      const streaming: StreamingMessage =
        current?.messageId === event.messageId
          ? { ...current, text: current.text + event.text, ...(phase && { phase }) }
          : { messageId: event.messageId, text: event.text, requestId: board.runRequest, startedAtMs: atMs, ...(phase && { phase }) };
      return { ...board, streaming };
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
      return { ...board, notices: [...board.notices, shownNotice(event.notice)].slice(-NOTICES) };
    case "taskUpdated":
      return { ...board, tasks: placed(board.tasks, event.task, envelope, board) };
    case "approvalUpdated":
      if (!shownApproval(event.approval)) return board;
      return { ...board, approvals: placed(board.approvals, event.approval, envelope, board) };
    case "questionUpdated":
      return { ...board, questions: placed(board.questions, event.question, envelope, board) };
    case "mcpServersUpdated": {
      const key = mcpAgentKey(event.taskId);
      const { [key]: _gone, ...others } = board.mcpServers;
      if (event.servers.length === 0) return key in board.mcpServers ? { ...board, mcpServers: others } : board;
      const agent: AgentMcpServers = { taskId: event.taskId, provider: event.provider, servers: event.servers, atStart: event.atStart };
      return { ...board, mcpServers: { ...others, [key]: agent } };
    }
    case "previewUpdated": {
      const previews = { ...board.previews, [event.preview.id]: event.preview };
      // Started again under its id, it is back in the Background rows.
      if (!previewActive(event.preview.state) || !board.backgroundCleared.includes(event.preview.id)) return { ...board, previews };
      return { ...board, previews, backgroundCleared: board.backgroundCleared.filter((id) => id !== event.preview.id) };
    }
    case "backgroundCleared":
      return { ...board, backgroundCleared: [...new Set([...board.backgroundCleared, ...event.ids])] };
    case "previewsCleared":
      return { ...board, previews: Object.fromEntries(Object.entries(board.previews).filter(([id, preview]) =>
        previewActive(preview.state) || !event.ids.includes(id),
      )) };
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
    case "peerMessageSent":
    case "peerMessageReceived": {
      const known = board.peerMessages[event.messageId];
      // Applied again after a view read: the record keeps what came after it.
      if (known) return board;
      const sent = event.type === "peerMessageSent";
      const message: PeerMessage = {
        messageId: event.messageId,
        outgoing: sent,
        peer: sent ? event.to : event.from,
        ...(event.replyTo !== null && { replyTo: event.replyTo }),
        requestId: event.requestId,
        state: sent ? event.state : { type: "pending" },
        text: sent ? event.text : "",
        blob: sent ? event.blob : null,
        sentAtMs: sent ? event.sentAtMs : null,
        settled: false,
        position: streamSeq,
        automatic: event.automatic,
        ...(event.answerId !== null && { answerId: event.answerId }),
      };
      return { ...board, peerMessages: { ...board.peerMessages, [event.messageId]: message } };
    }
    case "peerReplyHeld": {
      const known = board.peerMessages[event.messageId];
      if (!known) return board;
      return { ...board, peerMessages: { ...board.peerMessages, [event.messageId]: { ...known, hold: event.hold } } };
    }
    case "peerMessageState": {
      const known = board.peerMessages[event.messageId];
      if (!known) return board;
      const state = peerStateAfter(known.state, event.state);
      const sentAtMs = event.sentAtMs ?? known.sentAtMs;
      if (state === known.state && sentAtMs === known.sentAtMs) return board;
      return { ...board, peerMessages: { ...board.peerMessages, [event.messageId]: { ...known, state, sentAtMs } } };
    }
    case "peerWaitSettled": {
      const known = board.peerMessages[event.messageId];
      if (!known || known.settled) return board;
      return { ...board, peerMessages: { ...board.peerMessages, [event.messageId]: { ...known, settled: true } } };
    }
    case "compactionUpdated":
      return { ...board, compactions: placed(board.compactions, event.compaction, envelope, board) };
    case "messageRated":
      return { ...board, ratings: { ...board.ratings, [event.subject]: event.rating } };
    case "ratingCleared": {
      const { [event.subject]: _cleared, ...ratings } = board.ratings;
      return { ...board, ratings };
    }
    case "queueChanged":
      return { ...board, queue: event.queue };
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

/** What a CLI tool of the orchestrator is doing, by tool name. */
const TOOL_DOING: Readonly<Record<string, string>> = {
  WebSearch: "Searching the web",
  WebFetch: "Reading a web page",
};

/** What the orchestrator shows while its CLI retries a failed request on its own: "Reconnecting 2/5". */
const RETRY_WORDS = /^(Reconnecting|The model is busy, reconnecting)( \d+\/\d+)?$/;

/** Whether the orchestrator's activity is its CLI retrying on its own. */
export const RETRYING = { has: (doing: string | null): boolean => doing !== null && RETRY_WORDS.test(doing) };

/** The attempt a retry is on, from the provider's words ("retry 2 of 5", "2/5"): " 2/5", else "". */
function attemptOf(message: string): string {
  const found = /(?:retry|attempt|reconnecting)\D{0,12}?(\d+)\s*(?:of|\/)\s*(\d+)/i.exec(message);
  return found ? ` ${found[1]}/${found[2]}` : "";
}

/** The orchestrator's current activity after one of its provider events. */
export function doingOf(event: ProviderEvent, current: string | null): string | null {
  switch (event.type) {
    case "error":
      if (!event.error.willRetry) return current;
      return ["rateLimit", "overloaded", "usageLimit"].includes(event.error.kind)
        ? `The model is busy, reconnecting${attemptOf(event.error.message)}`
        : `Reconnecting${attemptOf(event.error.message)}`;
    case "toolCall": {
      if (event.status !== "inProgress") return null;
      return TOOL_DOING[event.name] ?? "Using a tool";
    }
    case "reasoning":
    case "message":
    case "turnCompleted":
    case "exited":
      return null;
    default:
      // Anything after a retry shows it went through.
      return RETRYING.has(current) ? null : current;
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
  for (const [id, board] of keptBoards) {
    const next = applyToShown(board, envelopes);
    if (next !== board) keptBoards.set(id, next);
  }
}

/** Whether an event is the board's: its conversation's, or one of its tasks'. */
function boardStream(board: Board, stream: string): boolean {
  if (stream === `conversation:${board.conversationId}`) return true;
  return stream.startsWith("task:") && !!board.tasks[stream.slice("task:".length)];
}

function applyToShown(board: Board, envelopes: readonly EventEnvelope[]): Board {
  let next = board;
  for (const envelope of envelopes) {
    if (boardStream(next, envelope.stream)) next = applyToBoard(next, envelope);
  }
  return next;
}

function applyToStore(store: BoardStore, envelopes: readonly EventEnvelope[]): void {
  const { board, orchestrator } = store.getState();
  if (!board && !orchestrator) return;
  const orchestratorStream = orchestrator ? `orch:${orchestrator.conversationId}` : null;
  let nextBoard = board;
  let nextLog = orchestrator;
  for (const envelope of envelopes) {
    const { stream } = envelope;
    if (nextBoard && boardStream(nextBoard, stream)) {
      nextBoard = applyToBoard(nextBoard, envelope);
      continue;
    }
    if (nextLog && stream === orchestratorStream) nextLog = applyToLog(nextLog, envelope);
  }
  if (nextBoard !== board || nextLog !== orchestrator) {
    store.setState({ board: nextBoard, orchestrator: nextLog });
  }
}

/**
 * What a CLI's live stream says of its conversation (`threadNews`), on its boards: how full its
 * model's context is, and what it is doing now.
 */
export function applyBoardNews(news: ThreadNews): void {
  if (!boardOf(news.conversationId) && !keptBoards.has(news.conversationId)) return;
  updateBoard(news.conversationId, (board) => {
    let doing = board.doing;
    for (const entry of news.entries) doing = doingOf(entry.event, doing);
    if (news.ended) doing = null;
    const context = news.context
      ? { usedTokens: news.context.usedTokens, windowTokens: news.context.windowTokens ?? board.context?.windowTokens ?? null }
      : board.context;
    return doing === board.doing && context === board.context ? board : { ...board, doing, context };
  });
}

/** Updates every board of the given conversation (the open one's, a side chat's, a kept one). */
export function updateBoard(conversationId: string, update: (board: Board) => Board): void {
  for (const store of [mainBoard, ...sideBoards]) {
    store.setState((state) =>
      state.board?.conversationId === conversationId ? { board: update(state.board) } : state,
    );
  }
  const kept = keptBoards.get(conversationId);
  if (kept) keptBoards.set(conversationId, update(kept));
}

/**
 * Puts a conversation's fresh view in its boards, or keeps it when none is there: read ahead of
 * showing it, it then shows at once.
 */
export function placeView(view: ConversationView, arrived: readonly EventEnvelope[]): void {
  const id = view.conversation.id;
  if (boardOf(id) || keptBoards.has(id)) updateBoard(id, (board) => boardFromView(view, board, arrived));
  else keepBoard(boardFromView(view, null, arrived));
}
