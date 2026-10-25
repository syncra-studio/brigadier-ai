import { doingOf, stepAtWork } from "@/app/conversation/activity/workerNow";
import type { BackgroundTask, StepAtWork, ThreadView, WorkerDoing, WorkerRow } from "@/ipc/generated";

/** Native agents share the Workers presentation, but never a Brigadier task's actions. */
export type NativeWorker = WorkerRow & {
  native: { conversationId: string; callId: string | null; backgroundId: string | null; prompt: string };
};

export function isNativeWorker(row: WorkerRow): row is NativeWorker {
  return "native" in row;
}

const cached = new WeakMap<ThreadView, readonly WorkerRow[]>();
const previous = new Map<string, readonly WorkerRow[]>();
const NO_ROWS: readonly WorkerRow[] = [];

/** Agent calls include foreground and completed agents; background task ids only drive Stop. */
export function workersOf(conversationId: string, view: ThreadView): readonly WorkerRow[] {
  const held = cached.get(view);
  if (held) return held;
  const rows: WorkerRow[] = [...(view.workers ?? [])];
  const tasks = (view.background ?? []).filter((task) => task.kind === "agent");
  const seen = new Set<string>();
  const lastChild = new Map<string, number>();
  // Each agent's calls at work, oldest first, as an engine worker's row lists its own.
  const atWork = new Map<string, StepAtWork[]>();
  // What each agent last did, as an engine worker's row says it between its calls (D39).
  const lastDone = new Map<string, WorkerDoing>();
  for (const entry of view.entries) {
    if (!entry.parent) continue;
    lastChild.set(entry.parent, entry.updatedAtMs ?? entry.atMs ?? 0);
    const step = stepAtWork(entry.event);
    if (step) atWork.set(entry.parent, [...(atWork.get(entry.parent) ?? []), step]);
    const doing = doingOf(entry.event);
    if (doing) lastDone.set(entry.parent, doing);
  }
  // A foreground agent works while its call does, in a turn still running: one its turn left
  // unfinished (the CLI ended or was stopped under it) is over.
  const last = view.turns.at(-1);
  const liveFrom = last && last.end === null ? (last.startedAtMs ?? 0) : Infinity;
  // Numbered after the engine's workers, so none shares a number.
  let next = Math.max(0, ...rows.map((row) => row.number)) + 1;
  const add = (callId: string | null, task: BackgroundTask | undefined, title: string, prompt: string, model: string | null, at: number, status: string, ended: number) => {
    const working = task ? task.status === "running" : status === "inProgress" && at >= liveFrom;
    const end = task ? task.status === "stopped" ? "stopped" : task.status === "failed" ? "failed" : "done"
      : status === "failed" || status === "declined" ? "failed" : "done";
    const steps = callId === null ? undefined : atWork.get(callId);
    const doing = callId === null ? undefined : lastDone.get(callId);
    const row: NativeWorker = {
      id: `native:${conversationId}:${callId ?? task!.id}`, number: next++, title,
      agent: "claude", model, startedAtMs: task?.startedAtMs ?? at,
      working, lastEnd: working ? null : end, gone: !working,
      lastEventAtMs: task?.endedAtMs ?? ended, limit: null, freshContext: false,
      waiting: [], mcp: [], branch: null, base: null, diff: null, ahead: 0, merged: false,
      ...(working && steps && { steps }),
      ...(working && doing && { doing }),
      native: { conversationId, callId, backgroundId: task?.id ?? null, prompt },
    };
    rows.push(row);
  };
  for (const entry of view.entries) {
    const event = entry.event;
    if (event.type !== "toolCall" || !["Agent", "Task"].includes(event.name)) continue;
    let input: Record<string, unknown> = {};
    try { input = JSON.parse(event.input ?? "{}") ?? {}; } catch { /* Streaming input is incomplete. */ }
    const task = tasks.find((candidate) => candidate.itemId === event.itemId);
    if (task) seen.add(task.id);
    const words = (key: string) => typeof input[key] === "string" ? input[key] as string : null;
    const at = entry.atMs ?? task?.startedAtMs ?? 0;
    add(event.itemId, task, words("description") ?? task?.label ?? words("name") ?? "Sub-agent",
      words("prompt") ?? "", words("model"), at, event.status, lastChild.get(event.itemId) ?? entry.updatedAtMs ?? at);
  }
  for (const task of tasks) {
    if (!seen.has(task.id)) add(task.itemId, task, task.label, "", null, task.startedAtMs, "completed", task.endedAtMs ?? task.startedAtMs);
  }
  // Ordinary token updates must not rerender every worker surface. Keep the engine's rows
  // directly when there are no native agents; otherwise reuse the unchanged presentation.
  const heldRows = previous.get(conversationId);
  const candidates = rows.length === (view.workers?.length ?? 0) ? view.workers ?? NO_ROWS
    : rows.toSorted((a, b) => b.startedAtMs - a.startedAtMs);
  const stable = heldRows && JSON.stringify(heldRows) === JSON.stringify(candidates) ? heldRows : candidates;
  previous.set(conversationId, stable);
  cached.set(view, stable);
  return stable;
}
