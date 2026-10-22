import { waitingWords } from "@/app/conversation/activity/peerWords";
import { isPlumbing } from "@/app/conversation/activity/words";
import { type BlockState, isFinal } from "@/app/conversation/blocks";
import { activelyWorking } from "@/app/conversation/taskActivity";
import type { QuotaWait, Task } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { type Board, RETRYING } from "@/state/board";

/**
 * The status line's tone: `busy` shimmers, `still` waits on something that is not the user,
 * `needsYou` waits on the user and shimmers as well.
 */
export type StatusTone = "busy" | "still" | "needsYou";

/**
 * `sinceMs`: a long run's start, its elapsed time said after the words ("Running pnpm test · 6m").
 * `waitsOn`: what of the user's the turn waits on: an answer's line carries a checklist glyph, an
 * approval's shimmers fainter.
 * `run`: a long run the request works on until its result comes, said under its answer as well.
 */
export type StatusHead = { text: string; tone: StatusTone; sinceMs?: number; waitsOn?: "answer" | "approval"; run?: true };

/**
 * What the end of a live turn says: one line. Each worker's own progress is in the Workers
 * strip on the composer, so it is never shown twice in the thread column.
 */
export type ThreadStatusView = { head: StatusHead | null };

export type StatusInput = {
  board: Pick<Board, "tasks" | "approvals" | "questions" | "plans" | "requests" | "run" | "runRequest" | "doing" | "streaming" | "orchestratorSteps" | "peerMessages" | "backgroundRuns">;
  requestIds: readonly string[];
  state: BlockState;
  /** A live thinking snippet shows at the end of the work. */
  thinkingLive: boolean;
  /** A compaction of the turn runs, its own row shimmering. */
  compacting: boolean;
  /** The conversation's messages wait for quota. */
  quotaWait: QuotaWait | null;
  /** The sessions whose replies the turn rests waiting for, by title. */
  waitingFor?: readonly string[];
};

function quotaWords(resetsAtMs: number | null): string {
  return resetsAtMs === null ? "Waiting for quota" : `Waiting for quota · resets ${formatTime(resetsAtMs)}`;
}

/** How long a run has gone, short: "40s", "6m", "1h 5m". */
export function elapsedWords(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** A command as a live line names it: its first line, cut short. */
export function commandWords(command: string): string {
  const line = command.trim().split("\n")[0] ?? "";
  return line.length > 40 ? `${line.slice(0, 39).trimEnd()}…` : line;
}

/**
 * A long run these requests wait on while the thread rests: a command of its own gone to the
 * background. The request works until its result comes, so the line says what runs and for how
 * long rather than "Thinking".
 */
function runningHead({ board, requestIds }: StatusInput): StatusHead | null {
  const run = Object.values(board.backgroundRuns)
    .filter((each) => each.state.type === "running" && each.requestId !== null && requestIds.includes(each.requestId))
    .toSorted((a, b) => a.startedAtMs - b.startedAtMs)[0];
  return run ? { text: `Running ${commandWords(run.command)}`, tone: "busy", sinceMs: run.startedAtMs, run: true } : null;
}

/**
 * The line under an answer whose request still waits: what waits on the user first, else the
 * sessions whose replies it waits for (also while the answer streams), else a long run the
 * request works on until its result comes, else what it waits on, or does once a late result
 * reopened it.
 */
export function answeredHead(head: StatusHead | null, { waitingFor, waiting, reopened }:
  { waitingFor: readonly string[]; waiting: boolean; reopened: boolean }): StatusHead | null {
  if (head?.tone === "needsYou") return head;
  if (waitingFor.length > 0) return { text: waitingWords(waitingFor), tone: "still" };
  if (head?.run) return head;
  // Reopened by a late result: it works again under its answer, so it says so (#107).
  return waiting || reopened ? head : null;
}

/** What only the user can do for these requests, in a few words; `null` when nothing. */
function needsYou({ board, requestIds }: StatusInput, leadRunning: boolean, workers: readonly Task[]): StatusHead | null {
  const ours = (id: string | null) => id !== null && requestIds.includes(id);
  // A call the user must allow: the turn's last row says so, quietly, the card itself asks.
  if (Object.values(board.approvals).some((card) => ours(card.requestId) && card.state.type === "pending")) {
    return { text: "Awaiting approval", tone: "needsYou", waitsOn: "approval" };
  }
  if (Object.values(board.questions).some((card) => ours(card.requestId) && card.answer === null && card.answeredAtMs === null)) {
    return { text: "Waiting for your answer", tone: "needsYou", waitsOn: "answer" };
  }
  // The user works in a worker's own session: the request waits until they close it.
  if (workers.some((task) => task.state === "takenOver")) return { text: "Working in your terminal", tone: "needsYou" };
  // The lead reviews a proposed plan and lands a ready change itself while its turn runs.
  if (leadRunning) return null;
  if (Object.values(board.plans).some((plan) => ours(plan.requestId) && plan.state.type === "proposed")
    || workers.some((task) => task.state === "readyToLand" && !task.run)) {
    return { text: "Waiting for your approval", tone: "needsYou" };
  }
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
  // A message on its way has its own row: "Sending message to API work".
  const sending = Object.values(board.peerMessages).some((message) => message.outgoing
    && message.requestId === runRequest && message.state.type === "pending");
  if (toolRunning || thinkingLive || compacting || streaming || sending) return null;
  if (board.doing) return { text: board.doing, tone: "busy" };
  return { text: board.run === "starting" ? "Starting" : "Thinking", tone: "busy" };
}

/**
 * The live line at the end of a turn: what the lead does right now, what waits on the user, or
 * how many workers it waits for. Nothing once the turn is over.
 */
export function threadStatus(input: StatusInput): ThreadStatusView {
  const { board, requestIds, state, quotaWait } = input;
  const none: ThreadStatusView = { head: null };
  if (state !== "working" && state !== "waiting") return none;
  const leadRunning = board.runRequest !== null && requestIds.includes(board.runRequest)
    && (board.run === "running" || board.run === "starting");
  const live = Object.values(board.tasks)
    .filter((task) => task.requestId !== null && requestIds.includes(task.requestId) && !isFinal(task))
    .toSorted((a, b) => a.createdAtMs - b.createdAtMs);
  const working = live.some((task) => activelyWorking({ task }));

  const needs = needsYou(input, leadRunning, live);
  if (needs) return { head: needs };
  if (leadRunning) return { head: leadHead(input) };
  if (quotaWait && !working) return { head: { text: quotaWords(quotaWait.resetsAtMs), tone: "still" } };
  if (live.length > 0) {
    const quota = live.every((task) => task.quotaWait);
    if (quota) {
      const resets = live.map((task) => task.quotaWait?.resetsAtMs ?? null).filter((at) => at !== null);
      return { head: { text: quotaWords(resets.length ? Math.min(...resets) : null), tone: "still" } };
    }
    if (live.every((task) => task.state === "landing")) return { head: { text: "Landing the changes", tone: "busy" } };
    // A worker that reported is done: the thread picks up its report next.
    const busy = live.filter((task) => task.state !== "reported");
    if (busy.length === 0) {
      const text = live.length === 1 ? "Worker finished, handing back" : `${live.length} workers finished, handing back`;
      return { head: { text, tone: "busy" } };
    }
    const text = busy.length === 1 ? "Waiting for a worker" : `Waiting for ${busy.length} workers`;
    return { head: { text, tone: working ? "busy" : "still" } };
  }
  const running = runningHead(input);
  if (running) return { head: running };
  // The lead's turn ended while a command it runs in the background goes on: that command's end
  // starts the turn that answers.
  const jobs = requestIds.reduce((sum, id) => sum + (board.requests[id]?.backgroundJobs ?? 0), 0);
  if (jobs > 0) {
    const text = jobs === 1 ? "Waiting for a command" : `Waiting for ${jobs} commands`;
    return { head: { text, tone: "still" } };
  }
  // It rests until other sessions reply: "Waiting for API work to reply".
  if (state === "waiting" && input.waitingFor?.length) return { head: { text: waitingWords(input.waitingFor), tone: "still" } };
  // Waiting with nothing above to show (an overnight run's list): no line. A question in the
  // lead's text waits for nothing; the user is asked on cards.
  if (state === "waiting") return none;
  // Between the lead's turns (a message just sent, a change being landed): it still works.
  return { head: { text: "Thinking", tone: "busy" } };
}
