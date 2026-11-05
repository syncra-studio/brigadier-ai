import { create } from "zustand";

import { request } from "@/ipc/client";
import type {
  ConversationActivity,
  EventEnvelope,
  McpServerState,
  RunState,
  TaskState,
  ThreadNews,
} from "@/ipc/generated";
import { readOrder } from "@/lib/readOrder";
import { forOrchestrator } from "@/state/board";
import { type Stop, stopsOf, useStopping } from "@/state/stopping";

/** A conversation's live state as the sidebar shows it, from the daemon's snapshot and events. */
type Activity = {
  run: RunState;
  /** Its CLI's turn is open. */
  turnOpen: boolean;
  /** One of its own workers has an open CLI turn, as the daemon's rows read it. */
  workersWorking?: boolean;
  /** Its CLI's background commands hold its turn, or one's end owes it a turn (D21). */
  commandsWorking?: boolean;
  /** Last recorded turn end, independent of git and requests. */
  turnEndedAtMs?: number;
  /** How its CLI's last turn ended, as its row names it: failed (or cut off), stopped, or neither. */
  ended: "failed" | "stopped" | null;
  /** Tasks not over yet, by id. */
  tasks: Readonly<Record<string, TaskState>>;
  /** Their titles, by id. */
  titles?: Readonly<Record<string, string>>;
  /** Its CLI's permission prompts waiting for the user. */
  approvals: ReadonlySet<string>;
  /** Unanswered questions. */
  questions: ReadonlySet<string>;
  /** Its CLI's MCP servers that want the user to sign in, by name. */
  signIns: ReadonlySet<string>;
};

const NONE: ReadonlySet<string> = new Set();

const IDLE: Activity = {
  run: "idle",
  turnOpen: false,
  workersWorking: false,
  commandsWorking: false,
  ended: null,
  tasks: {},
  titles: {},
  approvals: NONE,
  questions: NONE,
  signIns: NONE,
};

const FINAL: ReadonlySet<TaskState> = new Set(["landed", "done", "rejected", "stopped", "failed"]);
const WORKING: ReadonlySet<TaskState> = new Set([
  "queued",
  "starting",
  "running",
  "blocked",
  "landing",
]);

/**
 * The names of `servers` that want the user to sign in, as the engine's `sign_ins` reads them:
 * one whose sign-in the user manages outside the CLI is left out, many stay unsigned.
 */
export function signInsOf(servers: readonly McpServerState[]): string[] {
  return servers
    .filter((server) => server.status === "needs-auth" && !server.managed)
    .map((server) => server.name);
}

/**
 * Its work still holds while no turn of its CLI runs: one of its workers still works, or a
 * background command of its CLI still runs or owes the turn its end starts. A message sent then
 * waits in the queue while it is judged, as the daemon's `work_holds` reads it.
 */
export function workHolds(activity: Activity | undefined): boolean {
  if (!activity) return false;
  return !!activity.workersWorking || !!activity.commandsWorking
    || Object.values(activity.tasks).some((state) => WORKING.has(state) || state === "takenOver");
}

/** Its CLI works: its turn is open, Brigadier hands it a message, or its run goes on (a handover). */
export const works = (activity: Activity) => activity.turnOpen || activity.run === "starting" || activity.run === "running";

/**
 * Nothing works, and its CLI did not take the turn or its last turn failed or was cut off. A turn
 * a limit cut ends failed while Brigadier still runs it on, switching account or CLI: not yet.
 */
const failedOf = (activity: Activity) =>
  !works(activity) && (activity.run === "failed" || (activity.ended === "failed" && activity.run !== "hibernated"));

/** Nothing works, and the user stopped its CLI's last turn. */
export const stoppedOf = (activity: Activity | undefined): boolean =>
  !!activity && !works(activity) && activity.ended === "stopped";

type ActivityState = { byConversation: Record<string, Activity> };

/** Every conversation's activity as the daemon says it. */
const activityStore = create<ActivityState>(() => ({
  byConversation: {},
}));

/** `activity` as the user's stops under way show it: their workers over, a stopped turn ended. */
function stoppedActivity(activity: Activity, stops: readonly Stop[]): Activity {
  let shown = activity;
  for (const stop of stops) {
    const over = stop.tasks.filter((id) => id in shown.tasks);
    if (over.length > 0) {
      const tasks = { ...shown.tasks };
      const titles = { ...shown.titles };
      for (const id of over) {
        delete tasks[id];
        delete titles[id];
      }
      shown = { ...shown, tasks, titles };
    }
    if (stop.turn) {
      const running = shown.run === "running" || shown.run === "starting";
      shown = { ...shown, run: running ? "idle" : shown.run, turnOpen: false, ended: "stopped" };
    }
  }
  return shown;
}

const shownStates = new WeakMap<ActivityState, { stops: readonly Stop[]; state: ActivityState }>();

/** `state` as the stops under way show it; the same state when there are none. */
function shownState(state: ActivityState, stops: readonly Stop[]): ActivityState {
  if (stops.length === 0) return state;
  const cached = shownStates.get(state);
  if (cached?.stops === stops) return cached.state;
  const byConversation = { ...state.byConversation };
  for (const conversationId of new Set(stops.map((stop) => stop.conversationId))) {
    const activity = byConversation[conversationId];
    if (activity) byConversation[conversationId] = stoppedActivity(activity, stopsOf(stops, conversationId));
  }
  const shown = { byConversation };
  shownStates.set(state, { stops, state: shown });
  return shown;
}

/**
 * Every conversation's activity as the sidebar shows it: as the daemon says it, but with the
 * user's stops under way shown at once (#498). `setState` sets the daemon's.
 */
export const useActivity = Object.assign(
  function useActivity<T>(selector: (state: ActivityState) => T): T {
    const stops = useStopping((s) => s.stops);
    return activityStore((state) => selector(shownState(state, stops)));
  },
  {
    getState: (): ActivityState => shownState(activityStore.getState(), useStopping.getState().stops),
    setState: activityStore.setState,
    subscribe: (listener: () => void): (() => void) => {
      const own = activityStore.subscribe(listener);
      const stops = useStopping.subscribe(listener);
      return () => {
        own();
        stops();
      };
    },
  },
);

/** One conversation's activity as the daemon says it, stops under way or not. */
export function realActivity(conversationId: string): Activity | undefined {
  return activityStore.getState().byConversation[conversationId];
}

/** Calls `listener` on every change of the daemon's activity. */
export const onRealActivity = (listener: () => void): (() => void) => activityStore.subscribe(listener);

export function fromSnapshot(activity: ConversationActivity): Activity {
  // A failed run says itself; else a failure or a stop is its last turn's.
  const ended = activity.stopped ? "stopped" : activity.failed && activity.run !== "failed" ? "failed" : null;
  return {
    run: activity.run,
    turnOpen: activity.turnOpen ?? false,
    workersWorking: activity.workersWorking ?? false,
    commandsWorking: activity.commandsWorking ?? false,
    ...(activity.turnEndedAtMs === undefined ? {} : { turnEndedAtMs: activity.turnEndedAtMs }),
    ended,
    tasks: Object.fromEntries(activity.tasks),
    titles: Object.fromEntries(activity.titles ?? []),
    approvals: new Set(activity.approvals),
    questions: new Set(activity.questions),
    signIns: new Set(activity.signIns ?? []),
  };
}

/** What changed activity while a snapshot was read: events, or a piece of thread news. */
type Heard = { events: readonly EventEnvelope[] } | { news: ThreadNews };

/** The snapshot reads under way, each with what was heard since it was asked for (#758). */
const snapshotReads = new Set<Heard[]>();
/** Overlapping snapshot reads: one answering after a newer one was asked for is not applied. */
const snapshotOrder = readOrder<"activity">();

function heard(change: Heard): void {
  for (const read of snapshotReads) read.push(change);
}

/**
 * Reads every conversation's activity again (after connecting, missing events, or the window
 * showing). What was heard while it was read is applied again on top of it, in the order heard: a
 * turn that ended meanwhile stays ended, an approval answered meanwhile stays answered.
 */
export async function loadActivity(): Promise<void> {
  const current = snapshotOrder.read("activity");
  const since: Heard[] = [];
  snapshotReads.add(since);
  try {
    const { activity } = await request({ method: "getActivity" });
    if (!current()) return;
    let byConversation: Record<string, Activity> = Object.fromEntries(
      activity.map((entry) => [entry.conversationId, fromSnapshot(entry)]),
    );
    for (const change of since) {
      byConversation = ("events" in change ? foldEvents(byConversation, change.events) : foldNews(byConversation, change.news)) ?? byConversation;
    }
    activityStore.setState({ byConversation });
  } finally {
    snapshotReads.delete(since);
  }
}

function toggled(set: ReadonlySet<string>, id: string, on: boolean): ReadonlySet<string> {
  if (set.has(id) === on) return set;
  const next = new Set(set);
  if (on) next.add(id);
  else next.delete(id);
  return next;
}

function apply(current: Activity, { event }: EventEnvelope): Activity {
  switch (event.type) {
    case "runStateChanged":
      if (current.run === event.state) return current;
      // A session put to sleep has no turn open (D10).
      return event.state === "hibernated" ? { ...current, run: event.state, turnOpen: false } : { ...current, run: event.state };
    case "mcpServersUpdated": {
      // The session's own CLI's servers; a worker's are its own.
      if (event.taskId !== null) return current;
      const names = signInsOf(event.servers);
      const same = names.length === current.signIns.size && names.every((name) => current.signIns.has(name));
      return same ? current : { ...current, signIns: new Set(names) };
    }
    case "taskUpdated": {
      const { id, state, title } = event.task;
      if (FINAL.has(state)) {
        if (!(id in current.tasks)) return current;
        const { [id]: _over, ...tasks } = current.tasks;
        const { [id]: _gone, ...titles } = current.titles ?? {};
        return { ...current, tasks, titles };
      }
      if (current.tasks[id] === state && current.titles?.[id] === title) return current;
      return { ...current, tasks: { ...current.tasks, [id]: state }, titles: { ...current.titles, [id]: title } };
    }
    case "approvalUpdated":
      return {
        ...current,
        approvals: toggled(
          current.approvals,
          event.approval.id,
          event.approval.state.type === "pending" && !forOrchestrator(event.approval),
        ),
      };
    case "questionUpdated":
      return {
        ...current,
        questions: toggled(current.questions, event.question.id, event.question.answer === null),
      };
    default:
      return current;
  }
}

function conversationOf({ event }: EventEnvelope): string | null {
  switch (event.type) {
    case "runStateChanged":
    case "mcpServersUpdated":
      return event.conversationId;
    case "taskUpdated":
      return event.task.conversationId;
    case "approvalUpdated":
      return event.approval.conversationId;
    case "questionUpdated":
      return event.question.conversationId;
    default:
      return null;
  }
}

/** Folds a batch of events into the conversations' activity. */
export function applyActivityEvents(batch: readonly EventEnvelope[]): void {
  if (!batch.some((envelope) => conversationOf(envelope))) return;
  heard({ events: batch });
  const changed = foldEvents(activityStore.getState().byConversation, batch);
  if (changed) activityStore.setState({ byConversation: changed });
}

/** `byConversation` with `batch` folded in; `null` when nothing changed. */
function foldEvents(byConversation: Record<string, Activity>, batch: readonly EventEnvelope[]): Record<string, Activity> | null {
  let changed: Record<string, Activity> | null = null;
  for (const envelope of batch) {
    const id = conversationOf(envelope);
    if (!id) continue;
    const current = changed?.[id] ?? byConversation[id] ?? IDLE;
    const next = apply(current, envelope);
    if (next !== current) {
      changed ??= { ...byConversation };
      changed[id] = next;
    }
  }
  return changed;
}

/**
 * Folds session turns, daemon worker rows and what of its CLI's background work holds its turn
 * into activity, even when its thread has never opened.
 */
export function applyActivityNews(news: ThreadNews): void {
  if (!news.opened && !endedOf(news) && news.workers === undefined && news.jobs === undefined) return;
  // Applied again on a snapshot read meanwhile, a turn's end keeps the time it was heard at.
  const stamped = endedOf(news) && news.turnEndedAtMs === undefined ? { ...news, turnEndedAtMs: Date.now() } : news;
  heard({ news: stamped });
  const changed = foldNews(activityStore.getState().byConversation, stamped);
  if (changed) activityStore.setState({ byConversation: changed });
}

/** A turn handed over to another account or CLI goes on there: it never ended for the row (D180). */
const endedOf = (news: ThreadNews) => (news.ended === "handedOver" ? undefined : news.ended);

/** `byConversation` with `news` folded in; `null` when nothing changed. */
function foldNews(byConversation: Record<string, Activity>, news: ThreadNews): Record<string, Activity> | null {
  const ended = endedOf(news);
  const current = byConversation[news.conversationId] ?? IDLE;
  const turn: Activity = ended
    ? {
        ...current,
        turnOpen: false,
        turnEndedAtMs: news.turnEndedAtMs ?? Date.now(),
        ended: ended === "failed" || ended === "interrupted" ? "failed" : ended === "stopped" ? "stopped" : null,
      }
    : news.opened ? { ...current, turnOpen: true, ended: null } : current;
  const workers = news.workers === undefined ? turn : { ...turn, workersWorking: news.workers.some((worker) => worker.working) };
  const next = news.jobs === undefined ? workers : { ...workers, commandsWorking: !!news.jobs.holding?.length || !!news.jobs.waking || !!news.jobs.subagents };
  if (next.turnOpen === current.turnOpen && next.ended === current.ended && next.workersWorking === current.workersWorking
    && next.commandsWorking === current.commandsWorking && next.turnEndedAtMs === current.turnEndedAtMs) return null;
  return { ...byConversation, [news.conversationId]: next };
}

/**
 * What a sidebar row shows: a pill while it waits for the user (a permission prompt, a
 * question, an MCP sign-in, or its last turn failed), else a spinner while its CLI or one of its own workers works, or
 * a background command of its CLI's holds its turn (D21), and
 * how many of its workers run.
 */
export type RowActivity = {
  running: boolean;
  awaiting: "approval" | "input" | "signIn" | "failed" | null;
  workers: number;
};

const QUIET: RowActivity = { running: false, awaiting: null, workers: 0 };

export function rowActivity(activity: Activity | undefined): RowActivity {
  if (!activity) return QUIET;
  const failed = failedOf(activity);
  // Its turn failed while its work runs: its block is still open, so it reads Working until that
  // closes. A paused worker keeps the block open too (`work_holds`), though no message waits for it.
  const held = failed && (workHolds(activity) || Object.values(activity.tasks).includes("paused"));
  // Asleep, nothing of it runs (D10).
  const running = activity.run !== "hibernated" && (works(activity) || !!activity.workersWorking || !!activity.commandsWorking || held);
  const awaiting =
    activity.approvals.size > 0
      ? "approval"
      : activity.questions.size > 0
        ? "input"
        : activity.signIns.size > 0
          ? "signIn"
          : failed && !held
            ? "failed"
            : null;
  const workers = Object.values(activity.tasks).filter((state) => WORKING.has(state)).length;
  return running || awaiting ? { running, awaiting, workers } : QUIET;
}

/** The conversations whose CLI works now, for the menu-bar item. */
export function runningConversations(byConversation: Record<string, Activity>): string[] {
  return Object.entries(byConversation)
    .filter(([, activity]) => works(activity))
    .map(([id]) => id);
}

/** The row activity of one conversation; re-renders only when it changes. */
export function useRowActivity(conversationId: string): RowActivity {
  const running = useActivity((s) => rowActivity(s.byConversation[conversationId]).running);
  const awaiting = useActivity((s) => rowActivity(s.byConversation[conversationId]).awaiting);
  const workers = useActivity((s) => rowActivity(s.byConversation[conversationId]).workers);
  return running || awaiting ? { running, awaiting, workers } : QUIET;
}
