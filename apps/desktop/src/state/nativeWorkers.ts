import { agentDetail, isSubagentStart, toolName } from "@/app/conversation/activity/words";
import type { BackgroundTask, FileEntry, ThreadView, WorkerRow } from "@/ipc/generated";
import { CLIS } from "@/ipc/generated/clis";

/**
 * A CLI's own subagent as its panel shows it, over its call: a row of its parent's work, never
 * one of the workers (D96). `owner`: whose thread holds its call, the session's or a worker's.
 */
export type NativeWorker = WorkerRow & {
  native: { conversationId: string; owner: string; callId: string; backgroundId: string | null; prompt: string };
};

export function isNativeWorker(row: WorkerRow): row is NativeWorker {
  return "native" in row;
}

/** The panel of subagent `callId` in `owner`'s thread (the session's id, or a worker's). */
export const subagentPanel = (owner: string, callId: string) => `native:${owner}:${callId}`;

/** Whose thread and which call a subagent's panel is; `null` for any other panel. */
export function subagentPanelOf(panel: string | null | undefined): { owner: string; callId: string } | null {
  if (!panel?.startsWith("native:")) return null;
  const rest = panel.slice("native:".length);
  const at = rest.indexOf(":");
  return at > 0 ? { owner: rest.slice(0, at), callId: rest.slice(at + 1) } : null;
}

/**
 * Whether subagent call `entry` still works. A background one works while its task does, and
 * after it ended past its lead's last turn until the turn its end owes says whose it is
 * (`jobs.waking`), so its lead's work never folds between the two (D24). A foreground one works
 * while its call does, in a turn still running: one its turn left unfinished is over.
 */
function works(view: ThreadView, at: number, status: string, task: BackgroundTask | undefined): boolean {
  if (task) {
    const owedAfter = view.jobs?.waking ? (view.turns.findLast((turn) => turn.endedAtMs !== null)?.endedAtMs ?? Infinity) : Infinity;
    return task.status === "running" || (task.endedAtMs ?? -Infinity) >= owedAfter;
  }
  const last = view.turns.at(-1);
  return status === "inProgress" && at >= (last && last.end === null ? (last.startedAtMs ?? 0) : Infinity);
}

/** The thread's own subagent calls, with their background tasks (`backgroundOf`). */
function subagentCalls(view: ThreadView): { entry: FileEntry; task: BackgroundTask | undefined }[] {
  const background = backgroundOf(view);
  return view.entries.flatMap((entry) => {
    const event = entry.event;
    if (entry.parent || event.type !== "toolCall" || !isSubagentStart(event.name)) return [];
    return [{ entry, task: background.find((task) => task.kind === "agent" && task.itemId === event.itemId) }];
  });
}

const NO_TASKS: readonly BackgroundTask[] = [];
const lastBackground = new WeakMap<readonly FileEntry[], readonly BackgroundTask[]>();

/**
 * The thread's background tasks as its CLI lists them, plus a background task for each subagent
 * call its CLI lists none for that went on past its turn's own end (a CLI whose every subagent
 * runs on its own, the parent waiting only if it chooses): it ran in the background (D97). The
 * same list while nothing of it changes.
 */
export function backgroundOf(view: ThreadView): readonly BackgroundTask[] {
  const listed = view.background ?? NO_TASKS;
  const own = new Set(listed.map((task) => task.itemId));
  const outlived = view.entries.flatMap((entry): BackgroundTask[] => {
    const event = entry.event;
    if (entry.parent || event.type !== "toolCall" || !isSubagentStart(event.name) || own.has(event.itemId) || entry.atMs === undefined) return [];
    const at = entry.atMs;
    const turn = view.turns.findLast((each) => (each.startedAtMs ?? 0) <= at);
    if (turn?.end !== "done" || turn.endedAtMs === null) return [];
    const running = event.status === "inProgress";
    const ended = entry.updatedAtMs ?? at;
    if (!running && ended <= turn.endedAtMs) return [];
    return [{
      id: event.itemId, itemId: event.itemId, kind: "agent", label: agentDetail(event.name, event.input) ?? "Subagent",
      status: running ? "running" : event.status === "completed" ? "completed" : "failed", startedAtMs: at, endedAtMs: running ? null : ended,
      outputFile: null, summary: null, lastLine: null,
    }];
  });
  const all = withOwnJobs(outlived.length === 0 ? listed : [...listed, ...outlived], view.entries);
  if (all === listed) return listed;
  const held = lastBackground.get(view.entries);
  if (held && JSON.stringify(held) === JSON.stringify(all)) return held;
  lastBackground.set(view.entries, all);
  return all;
}

/** The subagent call whose own work `task` is (the call that started it runs inside one), if any. */
export function subagentOf(task: BackgroundTask, tasks: readonly BackgroundTask[], entries: readonly FileEntry[]): string | undefined {
  const parent = task.itemId === null ? undefined : entries.find(({ event }) => (event.type === "command" || event.type === "toolCall") && event.itemId === task.itemId)?.parent;
  return parent !== undefined && tasks.some((each) => each.kind === "agent" && each.itemId === parent) ? parent : undefined;
}

/**
 * A subagent works while a job it started still runs, though its CLI said it ended (Claude ends
 * the subagent's turn first and takes it up again once the job ends): its row, its Background
 * row, its block and the live line all read it running (D97). `tasks` itself when none is.
 */
function withOwnJobs(tasks: readonly BackgroundTask[], entries: readonly FileEntry[]): readonly BackgroundTask[] {
  const busy = new Set(tasks.flatMap((task) => (task.status === "running" ? [subagentOf(task, tasks, entries)] : [])));
  const still = (task: BackgroundTask) => task.kind === "agent" && task.status !== "running" && task.itemId !== null && busy.has(task.itemId);
  return tasks.some(still) ? tasks.map((task) => (still(task) ? { ...task, status: "running", endedAtMs: null } : task)) : tasks;
}

/**
 * How many of the subagents a turn that ran from `startedAtMs` to `endedAtMs` (null: still
 * running) started still work: while any does, its block stays open (D24).
 */
export function runningSubagents(view: ThreadView, startedAtMs: number, endedAtMs: number | null): number {
  return runningSubagentTasks(view, startedAtMs, endedAtMs).length;
}

/** The task of each subagent [`runningSubagents`] counts, as its Background row names it ("Subagent: <task>"). */
export function runningSubagentTasks(view: ThreadView, startedAtMs: number, endedAtMs: number | null): string[] {
  const calls = subagentCalls(view);
  const at = [
    ...calls.map(({ entry, task }) => ({
      at: entry.atMs ?? task?.startedAtMs ?? 0,
      status: entry.event.type === "toolCall" ? entry.event.status : "",
      task,
      label: task?.label || (entry.event.type === "toolCall" ? agentDetail(entry.event.name, entry.event.input) : null) || "a subagent",
    })),
    // A background agent whose call the thread doesn't hold (yet) counts from its start.
    ...backgroundOf(view).filter((task) => task.kind === "agent" && !calls.some((call) => call.task === task)).map((task) => ({ at: task.startedAtMs, status: "", task, label: task.label || "a subagent" })),
  ];
  return at.filter((call) => call.at >= startedAtMs && (endedAtMs === null || call.at <= endedAtMs) && works(view, call.at, call.status, call.task)).map((call) => call.label);
}

const shown = new Map<string, NativeWorker | null>();

/**
 * Subagent `callId` of `owner`'s thread `view`, as its panel's row; the same row while nothing
 * of it changes, so a streamed word elsewhere repaints none of its panel.
 */
export function subagentRow(conversationId: string, owner: string, view: ThreadView, callId: string): NativeWorker | null {
  const call = subagentCalls(view).find(({ entry }) => entry.event.type === "toolCall" && entry.event.itemId === callId);
  let row: NativeWorker | null = null;
  if (call?.entry.event.type === "toolCall") {
    const { entry, task } = call;
    const event = call.entry.event;
    let input: Record<string, unknown> = {};
    try { input = JSON.parse(event.input ?? "{}") ?? {}; } catch { /* Streaming input is incomplete. */ }
    const words = (key: string) => typeof input[key] === "string" ? input[key] as string : null;
    const at = entry.atMs ?? task?.startedAtMs ?? 0;
    const working = works(view, at, event.status, task);
    const end = task ? task.status === "stopped" ? "stopped" : task.status === "failed" ? "failed" : "done"
      : event.status === "failed" || event.status === "declined" ? "failed" : "done";
    const lastChild = view.entries.findLast((child) => child.parent === callId);
    const agent = CLIS.find((cli) => cli.subagentTools.includes(toolName(event.name)))?.id ?? task?.provider;
    if (agent) row = {
      id: subagentPanel(owner, callId), number: 0, title: agentDetail(event.name, event.input) ?? task?.label ?? words("name") ?? "Subagent",
      agent, model: words("model"), startedAtMs: task?.startedAtMs ?? at,
      working, lastEnd: working ? null : end, gone: !working,
      // Over, it ended when its call did, as its row says (D94); at work, at its last own item.
      lastEventAtMs: task?.endedAtMs ?? (working ? lastChild?.updatedAtMs ?? lastChild?.atMs : entry.updatedAtMs) ?? at, limit: null, freshContext: false,
      waiting: [], mcp: [], branch: null, base: null, diff: null, ahead: 0, merged: false,
      native: { conversationId, owner, callId, backgroundId: task?.id ?? null, prompt: words("prompt") ?? event.input ?? "" },
    };
  }
  const key = subagentPanel(owner, callId);
  const held = shown.get(key);
  if (held !== undefined && JSON.stringify(held) === JSON.stringify(row)) return held;
  shown.set(key, row);
  return row;
}
