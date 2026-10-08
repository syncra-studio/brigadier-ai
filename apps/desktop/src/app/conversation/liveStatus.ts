import { isPlumbing } from "@/app/conversation/activity/words";
import { type BlockState, isFinal } from "@/app/conversation/blocks";
import { activelyWorking } from "@/app/conversation/taskActivity";
import type { QuotaWait, Task } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { type Board, RETRYING } from "@/state/board";

/**
 * The status line's tone: `busy` shimmers, `still` waits on something that is not the user,
 * `needsYou` waits on the user.
 */
export type StatusTone = "busy" | "still" | "needsYou";

export type StatusHead = { text: string; tone: StatusTone };

/** What the end of a live turn says: a head line, then the request's workers still at it. */
export type ThreadStatusView = {
  head: StatusHead | null;
  /** The workers listed under the head, by task id, oldest first. */
  workers: string[];
  /** Workers at it beyond those listed. */
  more: number;
};

/** Workers listed by name under the head; the rest are counted. */
export const LISTED_WORKERS = 3;

export type StatusInput = {
  board: Pick<Board, "tasks" | "approvals" | "questions" | "plans" | "waiting" | "run" | "runRequest" | "doing" | "streaming" | "orchestratorSteps">;
  requestIds: readonly string[];
  state: BlockState;
  /** A live thinking snippet shows at the end of the work. */
  thinkingLive: boolean;
  /** A compaction of the turn runs, its own row shimmering. */
  compacting: boolean;
  /** The conversation's messages wait for quota. */
  quotaWait: QuotaWait | null;
};

function quotaWords(resetsAtMs: number | null): string {
  return resetsAtMs === null ? "Waiting for quota" : `Waiting for quota · resets ${formatTime(resetsAtMs)}`;
}

/** What only the user can do for these requests, in a few words; `null` when nothing. */
function needsYou({ board, requestIds, state }: StatusInput, leadRunning: boolean, workers: readonly Task[]): string | null {
  const ours = (id: string | null) => id !== null && requestIds.includes(id);
  if (Object.values(board.approvals).some((card) => ours(card.requestId) && card.state.type === "pending")) {
    return "Waiting for your approval";
  }
  if (Object.values(board.questions).some((card) => ours(card.requestId) && card.answer === null && card.answeredAtMs === null)) {
    return "Waiting for your answer";
  }
  const items = Object.values(board.waiting).filter((item) => ours(item.requestId));
  const [only] = items;
  if (only) return items.length === 1 ? `Waiting for you · ${only.what}` : "Waiting for you";
  // The user works in a worker's own session: the request waits until they close it.
  if (workers.some((task) => task.state === "takenOver")) return "Working in your terminal";
  // The lead reviews a proposed plan and lands a ready change itself while its turn runs.
  if (leadRunning) return null;
  if (Object.values(board.plans).some((plan) => ours(plan.requestId) && plan.state.type === "proposed")) {
    return "Waiting for your approval";
  }
  if (workers.some((task) => task.state === "readyToLand" && !task.run)) return "Waiting for your approval";
  // Waiting with no card: the lead asked in its reply, unless a worker's pause explains it.
  if (state === "waiting" && !workers.some((task) => task.state === "paused")) return "Waiting for your answer";
  return null;
}

/** What the lead's own turn shows at the end of the work, or `null` when its rows already do. */
function leadHead({ board, requestIds, thinkingLive, compacting }: StatusInput): StatusHead | null {
  // A retry stalls whatever streamed so far: say so over it.
  if (board.doing && RETRYING.has(board.doing)) return { text: board.doing, tone: "busy" };
  const runRequest = board.runRequest;
  // A plumbing call (one to a worker) has no row of its own: its "Delegating to a worker" shows here.
  const toolRunning = board.orchestratorSteps.some((step) => step.requestId === runRequest
    && step.kind.type === "tool" && step.kind.status === "inProgress" && !isPlumbing(step.kind.name));
  const streaming = !!board.streaming?.text && (board.streaming.requestId === null || requestIds.includes(board.streaming.requestId));
  if (toolRunning || thinkingLive || compacting || streaming) return null;
  if (board.doing) return { text: board.doing, tone: "busy" };
  return { text: board.run === "starting" ? "Starting" : "Thinking", tone: "busy" };
}

/**
 * The live line at the end of a turn: what the lead does right now, what waits on the user, or
 * which workers it waits for. Nothing once the turn is over.
 */
export function threadStatus(input: StatusInput): ThreadStatusView {
  const { board, requestIds, state, quotaWait } = input;
  const none: ThreadStatusView = { head: null, workers: [], more: 0 };
  if (state !== "working" && state !== "waiting") return none;
  const leadRunning = board.runRequest !== null && requestIds.includes(board.runRequest)
    && (board.run === "running" || board.run === "starting");
  const live = Object.values(board.tasks)
    .filter((task) => task.requestId !== null && requestIds.includes(task.requestId) && !isFinal(task))
    .toSorted((a, b) => a.createdAtMs - b.createdAtMs);
  const workers = live.slice(0, LISTED_WORKERS).map((task) => task.id);
  const more = Math.max(0, live.length - LISTED_WORKERS);
  const working = live.some((task) => activelyWorking({ task }));

  const needs = needsYou(input, leadRunning, live);
  if (needs) return { head: { text: needs, tone: "needsYou" }, workers, more };
  if (leadRunning) return { head: leadHead(input), workers, more };
  if (quotaWait && !working) return { head: { text: quotaWords(quotaWait.resetsAtMs), tone: "still" }, workers, more };
  if (live.length > 0) {
    const quota = live.every((task) => task.quotaWait);
    if (quota) {
      const resets = live.map((task) => task.quotaWait?.resetsAtMs ?? null).filter((at) => at !== null);
      return { head: { text: quotaWords(resets.length ? Math.min(...resets) : null), tone: "still" }, workers, more };
    }
    if (live.every((task) => task.state === "landing")) return { head: { text: "Landing the changes", tone: "busy" }, workers: [], more: 0 };
    // A worker that reported is done: the thread picks up its report next.
    const busy = live.filter((task) => task.state !== "reported");
    if (busy.length === 0) {
      const text = live.length === 1 ? "Worker finished, handing back" : `${live.length} workers finished, handing back`;
      return { head: { text, tone: "busy" }, workers, more };
    }
    const text = busy.length === 1 ? "Waiting for a worker" : `Waiting for ${busy.length} workers`;
    return { head: { text, tone: working ? "busy" : "still" }, workers, more };
  }
  // Between the lead's turns (a message just sent, a change being landed): it still works.
  return { head: { text: "Thinking", tone: "busy" }, workers, more };
}
