import { create } from "zustand";

import { request } from "@/ipc/client";
import type {
  ConversationActivity,
  EventEnvelope,
  OrchestratorStepKind,
  RunState,
  SessionPhase,
  TaskKind,
  TaskState,
} from "@/ipc/generated";

/** A conversation's live state as the sidebar shows it, from the daemon's snapshot and events. */
type Activity = {
  run: RunState;
  /** Tasks not over yet, by id. */
  tasks: Readonly<Record<string, TaskState>>;
  /** Their titles, by id. */
  titles?: Readonly<Record<string, string>>;
  /** Approvals and plans waiting for the user. */
  approvals: ReadonlySet<string>;
  /** Unanswered questions. */
  questions: ReadonlySet<string>;
  /** What it is busy with that its row names, keyed by what runs it (`ci:`, `merge:`, …). */
  phases: Readonly<Record<string, SessionPhase>>;
  /** Its latest request or its run failed, and nothing ran since. */
  failed: boolean;
  /** Its latest request was stopped by the user, until another turn starts. */
  stopped?: boolean;
};

const IDLE: Activity = {
  run: "idle",
  tasks: {},
  titles: {},
  approvals: new Set(),
  questions: new Set(),
  phases: {},
  failed: false,
  stopped: false,
};

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
/** The phase a working task of each kind puts its conversation in. */
const TASK_PHASES: Partial<Record<TaskKind, SessionPhase>> = {
  merge: "merging",
  review: "reviewing",
  verify: "checking",
};
/** The thread's calls that put its conversation in a phase while they run. */
const TOOL_PHASES: Readonly<Record<string, SessionPhase>> = {
  run_check: "checking",
  finish_session: "merging",
};
/** Which phase a row names when several run, first first. */
const PHASE_ORDER: readonly SessionPhase[] = ["merging", "reviewing", "checking", "watchingCi"];

export const useActivity = create<{ byConversation: Record<string, Activity> }>(() => ({
  byConversation: {},
}));

export function fromSnapshot(activity: ConversationActivity): Activity {
  return {
    run: activity.run,
    tasks: Object.fromEntries(activity.tasks),
    titles: Object.fromEntries(activity.titles ?? []),
    approvals: new Set(activity.approvals),
    questions: new Set(activity.questions),
    phases: Object.fromEntries(activity.phases),
    failed: activity.failed ?? false,
    stopped: activity.stopped ?? false,
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

/** `current` with the phase under `key` set, or cleared when `phase` is none. */
function phased(current: Activity, key: string, phase: SessionPhase | undefined): Activity {
  if (current.phases[key] === phase) return current;
  const { [key]: _old, ...phases } = current.phases;
  return { ...current, phases: phase ? { ...phases, [key]: phase } : phases };
}

/** `current` without the phases whose key starts with `prefix`. */
function unphased(current: Activity, prefix: string): Activity {
  const keys = Object.keys(current.phases).filter((key) => key.startsWith(prefix));
  if (keys.length === 0) return current;
  const phases = { ...current.phases };
  for (const key of keys) delete phases[key];
  return { ...current, phases };
}

function stepped(current: Activity, kind: OrchestratorStepKind): Activity {
  switch (kind.type) {
    case "ciWatch":
      return phased(
        current,
        `ci:${kind.repo} ${kind.branch} ${kind.sha}`,
        kind.result == null ? "watchingCi" : undefined,
      );
    case "mergeHeld":
      // Only the latest hold is pending: a fresh consent replaces the one held before.
      return phased(unphased(current, "merge:"), `merge:${kind.askedIn}`, "merging");
    case "merged":
      return unphased(current, "merge:");
    case "mergeCancelled":
    case "mergeStopped":
      return phased(current, `merge:${kind.askedIn}`, undefined);
    case "tool": {
      const phase = TOOL_PHASES[kind.name.split("__").at(-1) ?? ""];
      return phase
        ? phased(current, `tool:${kind.itemId}`, kind.status === "inProgress" ? phase : undefined)
        : current;
    }
    default:
      return current;
  }
}

function apply(current: Activity, { event }: EventEnvelope): Activity {
  switch (event.type) {
    case "runStateChanged": {
      const running = event.state === "running" || event.state === "starting";
      const failed = event.state === "failed" ? true : running ? false : current.failed;
      const next = { ...current, run: event.state, failed, stopped: running ? false : (current.stopped ?? false) };
      // A call the turn left in progress isn't running any more.
      return running ? next : unphased(next, "tool:");
    }
    case "requestUpdated": {
      const { type } = event.request.state;
      const failed = type === "failed" ? true : type === "working" ? false : current.failed;
      const stopped = type === "stopped";
      return failed === current.failed && stopped === current.stopped ? current : { ...current, failed, stopped };
    }
    case "taskUpdated": {
      const { id, state, kind, title } = event.task;
      const next = phased(
        current,
        `task:${id}`,
        WORKING.has(state) ? TASK_PHASES[kind] : undefined,
      );
      if (FINAL.has(state)) {
        if (!(id in next.tasks)) return next;
        const { [id]: _over, ...tasks } = next.tasks;
        const { [id]: _gone, ...titles } = next.titles ?? {};
        return { ...next, tasks, titles };
      }
      if (next.tasks[id] === state && next.titles?.[id] === title) return next;
      return { ...next, tasks: { ...next.tasks, [id]: state }, titles: { ...next.titles, [id]: title } };
    }
    case "reviewUpdated":
      return phased(
        current,
        `review:${event.review.id}`,
        event.review.state.type === "running" ? "reviewing" : undefined,
      );
    case "integratedCheckUpdated":
      return phased(
        current,
        `check:${event.check.id}`,
        event.check.state.type === "running" ? "checking" : undefined,
      );
    case "backgroundRunUpdated":
      return event.run.tool === "run_check"
        ? phased(
            current,
            `run:${event.run.id}`,
            event.run.state.type === "running" ? "checking" : undefined,
          )
        : current;
    case "orchestratorStepped":
      return stepped(current, event.step.kind);
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

function conversationOf({ stream, event }: EventEnvelope): string | null {
  switch (event.type) {
    case "orchestratorStepped":
      return stream.startsWith("conversation:") ? stream.slice("conversation:".length) : null;
    case "reviewUpdated":
      return event.review.conversationId;
    case "integratedCheckUpdated":
      return event.check.conversationId;
    case "backgroundRunUpdated":
      return event.run.conversationId;
    case "runStateChanged":
      return event.conversationId;
    case "requestUpdated":
      return event.request.conversationId;
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

/**
 * What a sidebar row shows: a pill while it waits for the user (a card, a question, or its last
 * turn failed), else a spinner while it runs or is in a phase, with the phase's name, and how
 * many of its workers run.
 */
export type RowActivity = {
  running: boolean;
  awaiting: "approval" | "input" | "failed" | null;
  phase: SessionPhase | null;
  workers: number;
};

const QUIET: RowActivity = { running: false, awaiting: null, phase: null, workers: 0 };

export function rowActivity(activity: Activity | undefined): RowActivity {
  if (!activity) return QUIET;
  const states = Object.values(activity.tasks);
  const turn = activity.run === "running" || activity.run === "starting";
  const running = turn || states.some((state) => WORKING.has(state));
  // A failed turn waits for the user even while its workers still run, as the engine says.
  const awaiting =
    activity.approvals.size > 0 || states.some((state) => LANDING.has(state))
      ? "approval"
      : activity.questions.size > 0
        ? "input"
        : activity.failed && !turn
          ? "failed"
          : null;
  const phases = new Set(Object.values(activity.phases));
  const phase = PHASE_ORDER.find((each) => phases.has(each)) ?? null;
  const workers = states.filter((state) => WORKING.has(state)).length;
  return running || awaiting || phase ? { running, awaiting, phase, workers } : QUIET;
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
  const phase = useActivity((s) => rowActivity(s.byConversation[conversationId]).phase);
  const workers = useActivity((s) => rowActivity(s.byConversation[conversationId]).workers);
  return running || awaiting || phase ? { running, awaiting, phase, workers } : QUIET;
}
