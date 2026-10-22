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
import { type Stop, stopsOf, useStopping } from "@/state/stopping";

/** A conversation's live state as the sidebar shows it, from the daemon's snapshot and events. */
type Activity = {
  run: RunState;
  /** Its CLI's turn is open. */
  turnOpen: boolean;
  /** How its CLI's last turn ended, as its row names it: failed (or cut off), stopped, or neither. */
  ended: "failed" | "stopped" | null;
  /** Tasks not over yet, by id. */
  tasks: Readonly<Record<string, TaskState>>;
  /** Their titles, by id. */
  titles?: Readonly<Record<string, string>>;
  /** Its CLI's permission prompts and plans waiting for the user. */
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
 * a claude.ai connector is left out, the user manages those on claude.ai and many stay unsigned.
 */
export function signInsOf(servers: readonly McpServerState[]): string[] {
  return servers
    .filter((server) => server.status === "needs-auth" && !server.name.startsWith("claude.ai "))
    .map((server) => server.name);
}

/** Its CLI works: its turn is open, or Brigadier hands it a message. */
const works = (activity: Activity) => activity.turnOpen || activity.run === "starting";

/** Nothing works, and its CLI did not take the turn or its last turn failed or was cut off. */
const failedOf = (activity: Activity) => !works(activity) && (activity.run === "failed" || activity.ended === "failed");

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
    ended,
    tasks: Object.fromEntries(activity.tasks),
    titles: Object.fromEntries(activity.titles ?? []),
    approvals: new Set(activity.approvals),
    questions: new Set(activity.questions),
    signIns: new Set(activity.signIns ?? []),
  };
}

/** Reads every conversation's activity again (after connecting or missing events). */
export async function loadActivity(): Promise<void> {
  const { activity } = await request({ method: "getActivity" });
  activityStore.setState({
    byConversation: Object.fromEntries(
      activity.map((entry) => [entry.conversationId, fromSnapshot(entry)]),
    ),
  });
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
      return current.run === event.state ? current : { ...current, run: event.state };
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
          event.approval.state.type === "pending",
        ),
      };
    case "planUpdated":
      return {
        ...current,
        approvals: toggled(current.approvals, event.plan.id, event.plan.state.type === "proposed"),
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
    case "planUpdated":
      return event.plan.conversationId;
    case "questionUpdated":
      return event.question.conversationId;
    default:
      return null;
  }
}

/** Folds a batch of events into the conversations' activity. */
export function applyActivityEvents(batch: readonly EventEnvelope[]): void {
  let changed: Record<string, Activity> | null = null;
  const { byConversation } = activityStore.getState();
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
  if (changed) activityStore.setState({ byConversation: changed });
}

/** Folds a CLI's turn opening or ending (`threadNews`) into its conversation's activity. */
export function applyActivityNews(news: ThreadNews): void {
  if (!news.opened && !news.ended) return;
  const { byConversation } = activityStore.getState();
  const current = byConversation[news.conversationId] ?? IDLE;
  const next: Activity = news.ended
    ? {
        ...current,
        turnOpen: false,
        ended: news.ended === "failed" || news.ended === "interrupted" ? "failed" : news.ended === "stopped" ? "stopped" : null,
      }
    : { ...current, turnOpen: true, ended: null };
  if (next.turnOpen === current.turnOpen && next.ended === current.ended) return;
  activityStore.setState({ byConversation: { ...byConversation, [news.conversationId]: next } });
}

/**
 * What a sidebar row shows: a pill while it waits for the user (a permission prompt or plan, a
 * question, an MCP sign-in, or its last turn failed), else a spinner while its CLI works, and
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
  const running = works(activity);
  const awaiting =
    activity.approvals.size > 0
      ? "approval"
      : activity.questions.size > 0
        ? "input"
        : activity.signIns.size > 0
          ? "signIn"
          : failedOf(activity)
            ? "failed"
            : null;
  const workers = Object.values(activity.tasks).filter((state) => WORKING.has(state)).length;
  return running || awaiting ? { running, awaiting, workers } : QUIET;
}

/** The conversations whose CLI works now, for the menu-bar item. */
export function runningConversations(byConversation: Record<string, Activity>): string[] {
  return Object.entries(byConversation)
    .filter(([, activity]) => rowActivity(activity).running)
    .map(([id]) => id);
}

/** The row activity of one conversation; re-renders only when it changes. */
export function useRowActivity(conversationId: string): RowActivity {
  const running = useActivity((s) => rowActivity(s.byConversation[conversationId]).running);
  const awaiting = useActivity((s) => rowActivity(s.byConversation[conversationId]).awaiting);
  const workers = useActivity((s) => rowActivity(s.byConversation[conversationId]).workers);
  return running || awaiting ? { running, awaiting, workers } : QUIET;
}
