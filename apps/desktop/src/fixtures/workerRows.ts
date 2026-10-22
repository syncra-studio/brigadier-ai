/**
 * Fixtures' worker rows (S14): a session's workers as the engine sends them with its thread
 * (`ThreadView.workers`), made from a fixture's tasks, and a worker's own thread as `workerThread`
 * answers it.
 */
import type { DiffStat, RawEntry, Task, ThreadView, TurnEnd, WorkerRow } from "@/ipc/generated";
import { useCliThread } from "@/state/cliThread";

const ENDS: Partial<Record<Task["state"], TurnEnd>> = {
  reported: "done",
  landed: "done",
  done: "done",
  stopped: "stopped",
  rejected: "stopped",
  failed: "failed",
};

const FINAL: ReadonlySet<Task["state"]> = new Set(["landed", "done", "rejected", "stopped", "failed"]);

/** A task as its row: at work while it runs, its end by how it finished, its +/− as given. */
export function rowOf(task: Task, patch: Partial<WorkerRow> = {}, diff: DiffStat | null = null): WorkerRow {
  const lastEnd = ENDS[task.state] ?? null;
  return {
    id: task.id,
    number: task.number,
    title: task.title,
    agent: task.route?.choice.provider ?? "claude",
    model: task.route?.choice.model ?? null,
    startedAtMs: task.createdAtMs,
    working: ["starting", "running", "landing"].includes(task.state),
    lastEnd,
    gone: FINAL.has(task.state),
    lastEventAtMs: task.updatedAtMs,
    limit: null,
    freshContext: false,
    waiting: [],
    mcp: [],
    branch: null,
    base: null,
    diff,
    ahead: 0,
    merged: task.state === "landed",
    ...patch,
  };
}

const EMPTY: ThreadView = { entries: [], turns: [] };

/** Puts `rows` in session `conversationId`'s thread view, newest first as the engine sends them. */
export function showRows(conversationId: string, rows: readonly WorkerRow[], view: Partial<ThreadView> = {}): void {
  useCliThread.setState((state) => ({
    views: {
      ...state.views,
      [conversationId]: { ...EMPTY, ...state.views[conversationId], ...view, workers: rows.toSorted((a, b) => b.startedAtMs - a.startedAtMs) },
    },
  }));
}

/** A worker's transcript as its own thread (`workerThread`): its entries, in order. */
export function threadOf(entries: readonly RawEntry[]): ThreadView {
  return { entries: entries.map((entry) => ({ event: entry.event, atMs: entry.atMs })), turns: [] };
}
