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

/** The thread's own subagent calls, with their background tasks. */
function subagentCalls(view: ThreadView): { entry: FileEntry; task: BackgroundTask | undefined }[] {
  return view.entries.flatMap((entry) => {
    const event = entry.event;
    if (entry.parent || event.type !== "toolCall" || !isSubagentStart(event.name)) return [];
    return [{ entry, task: view.background?.find((task) => task.kind === "agent" && task.itemId === event.itemId) }];
  });
}

/**
 * How many of the subagents a turn that ran from `startedAtMs` to `endedAtMs` (null: still
 * running) started still work: while any does, its block stays open (D24).
 */
export function runningSubagents(view: ThreadView, startedAtMs: number, endedAtMs: number | null): number {
  const calls = subagentCalls(view);
  const at = [
    ...calls.map(({ entry, task }) => ({ at: entry.atMs ?? task?.startedAtMs ?? 0, status: entry.event.type === "toolCall" ? entry.event.status : "", task })),
    // A background agent whose call the thread doesn't hold (yet) counts from its start.
    ...(view.background ?? []).filter((task) => task.kind === "agent" && !calls.some((call) => call.task === task)).map((task) => ({ at: task.startedAtMs, status: "", task })),
  ];
  return at.filter((call) => call.at >= startedAtMs && (endedAtMs === null || call.at <= endedAtMs) && works(view, call.at, call.status, call.task)).length;
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
