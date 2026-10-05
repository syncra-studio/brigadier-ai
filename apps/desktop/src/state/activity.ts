import { create } from "zustand";

import { request } from "@/ipc/client";
import type {
  ConversationActivity,
  EventEnvelope,
  RunState,
  TaskState,
} from "@/ipc/generated";

/** A conversation's live state as the sidebar shows it, from the daemon's snapshot and events. */
type Activity = {
  run: RunState;
  /** Tasks not over yet, by id. */
  tasks: Readonly<Record<string, TaskState>>;
  /** Approvals and plans waiting for the user. */
  approvals: ReadonlySet<string>;
  /** Unanswered questions. */
  questions: ReadonlySet<string>;
};

const IDLE: Activity = { run: "idle", tasks: {}, approvals: new Set(), questions: new Set() };

const FINAL: ReadonlySet<TaskState> = new Set(["landed", "done", "rejected", "stopped", "failed"]);
const WORKING: ReadonlySet<TaskState> = new Set([
  "queued",
  "starting",
  "running",
  "blocked",
  "landing",
]);
/** A task that waits for the user to approve its landing. */
const LANDING: ReadonlySet<TaskState> = new Set(["readyToLand"]);

export const useActivity = create<{ byConversation: Record<string, Activity> }>(() => ({
  byConversation: {},
}));

function fromSnapshot(activity: ConversationActivity): Activity {
  return {
    run: activity.run,
    tasks: Object.fromEntries(activity.tasks),
    approvals: new Set(activity.approvals),
    questions: new Set(activity.questions),
  };
}

/** Reads every conversation's activity again (after connecting or missing events). */
export async function loadActivity(): Promise<void> {
  const { activity } = await request({ method: "getActivity" });
  useActivity.setState({
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
      return { ...current, run: event.state };
    case "taskUpdated": {
      const { id, state } = event.task;
      if (FINAL.has(state)) {
        if (!(id in current.tasks)) return current;
        const { [id]: _over, ...tasks } = current.tasks;
        return { ...current, tasks };
      }
      return current.tasks[id] === state
        ? current
        : { ...current, tasks: { ...current.tasks, [id]: state } };
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
  const { byConversation } = useActivity.getState();
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
  if (changed) useActivity.setState({ byConversation: changed });
}

/** What a sidebar row shows: a spinner while it runs, a pill while it waits for the user. */
export type RowActivity = { running: boolean; awaiting: "approval" | "input" | null };

const QUIET: RowActivity = { running: false, awaiting: null };

function rowActivity(activity: Activity | undefined): RowActivity {
  if (!activity) return QUIET;
  const states = Object.values(activity.tasks);
  const running =
    activity.run === "running" ||
    activity.run === "starting" ||
    states.some((state) => WORKING.has(state));
  const awaiting =
    activity.approvals.size > 0 || states.some((state) => LANDING.has(state))
      ? "approval"
      : activity.questions.size > 0
        ? "input"
        : null;
  return running || awaiting ? { running, awaiting } : QUIET;
}

/** The row activity of one conversation; re-renders only when it changes. */
/** The conversations running now (a turn or a worker in progress), for the menu-bar item. */
export function runningConversations(byConversation: Record<string, Activity>): string[] {
  return Object.entries(byConversation)
    .filter(([, activity]) => rowActivity(activity).running)
    .map(([id]) => id);
}

export function useRowActivity(conversationId: string): RowActivity {
  const running = useActivity((s) => rowActivity(s.byConversation[conversationId]).running);
  const awaiting = useActivity((s) => rowActivity(s.byConversation[conversationId]).awaiting);
  return running || awaiting ? { running, awaiting } : QUIET;
}
