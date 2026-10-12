import { create } from "zustand";

import { exportConventions, request, revealPath } from "@/ipc/client";
import type {
  BrainAnswer,
  BrainGraph,
  BrainJob,
  BrainJobKind,
  BrainOverview,
  BrainQuery,
  EventEnvelope,
  Node,
  NodeFilter,
  Task,
} from "@/ipc/generated";
import { toast } from "@/state/toasts";

/** The Personal Brain's key in the Brain tab's picker; a project's Brain uses the project id. */
export const PERSONAL = "personal";

/** The daemon's `projectId` for a picker key. */
export function projectOf(key: string): string | null {
  return key === PERSONAL ? null : key;
}

type BrainState = {
  /** The Brain the Inspector's Brain tab shows; `null` until the user picks one. */
  selected: string | null;
  /** Overviews read so far, by picker key, kept live by Brain job events. */
  overviews: Record<string, BrainOverview>;
  /**
   * The tasks of the session the Brain tab lists workers for: one snapshot, then kept current
   * by task events, since Settings (where the Inspector lives) holds no board.
   */
  sessionTasks: { conversationId: string; tasks: Record<string, Task> } | null;
};

export const useBrain = create<BrainState>()(() => ({ selected: null, overviews: {}, sessionTasks: null }));

export function selectBrain(key: string): void {
  useBrain.setState({ selected: key });
}

export async function loadBrain(key: string): Promise<BrainOverview> {
  const { overview } = await request({ method: "getBrain", projectId: projectOf(key) });
  useBrain.setState((state) => ({ overviews: { ...state.overviews, [key]: overview } }));
  return overview;
}

function upsertJob(jobs: readonly BrainJob[], job: BrainJob): BrainJob[] {
  const others = jobs.filter((existing) => existing.id !== job.id);
  return [job, ...others].toSorted((a, b) => b.startedAtMs - a.startedAtMs);
}

/** Keeps the newer copy of each task; events arrive in order, so the later one wins a tie. */
function applyTasks(known: Record<string, Task>, tasks: readonly Task[]): Record<string, Task> {
  const merged = { ...known };
  for (const task of tasks) {
    const held = merged[task.id];
    if (!held || held.updatedAtMs <= task.updatedAtMs) merged[task.id] = task;
  }
  return merged;
}

/** Session task reads in flight, each collecting the task updates that arrive meanwhile. */
const taskReads = new Set<{ conversationId: string; tasks: Task[] }>();

/**
 * Lists a session's workers for the Brain tab: reads its tasks once (and again when the bridge
 * resyncs, on reconnecting or after missing events); task events keep them current in between.
 * A read replaces what was held, so an update the feed missed comes back even when it tied with
 * one it delivered; updates that arrived while it was read are applied again, since the read
 * may predate them.
 */
export async function watchSessionTasks(conversationId: string): Promise<void> {
  if (useBrain.getState().sessionTasks?.conversationId !== conversationId) {
    useBrain.setState({ sessionTasks: { conversationId, tasks: {} } });
  }
  const read = { conversationId, tasks: [] as Task[] };
  taskReads.add(read);
  let view;
  try {
    ({ view } = await request({ method: "getConversation", id: conversationId, limit: 1 }));
  } finally {
    taskReads.delete(read);
  }
  const snapshot = Object.fromEntries(view.tasks.map((task) => [task.id, task]));
  useBrain.setState((state) =>
    state.sessionTasks?.conversationId === conversationId
      ? { sessionTasks: { conversationId, tasks: applyTasks(snapshot, read.tasks) } }
      : state,
  );
}

/** Reads the watched session's tasks again: the bridge's resync, after events may have been missed. */
export async function reloadSessionTasks(): Promise<void> {
  const watched = useBrain.getState().sessionTasks;
  if (watched) await watchSessionTasks(watched.conversationId);
}

export function unwatchSessionTasks(conversationId: string): void {
  if (useBrain.getState().sessionTasks?.conversationId === conversationId) {
    useBrain.setState({ sessionTasks: null });
  }
}

/** Folds the watched session's task updates into its tasks (one update per batch). */
function applyTaskEvents(envelopes: readonly EventEnvelope[]): void {
  const updates = envelopes.flatMap(({ event }) => (event.type === "taskUpdated" ? [event.task] : []));
  for (const read of taskReads) {
    read.tasks.push(...updates.filter((task) => task.conversationId === read.conversationId));
  }
  const watched = useBrain.getState().sessionTasks;
  if (!watched) return;
  const tasks = updates.filter((task) => task.conversationId === watched.conversationId);
  if (tasks.length === 0) return;
  useBrain.setState((state) =>
    state.sessionTasks?.conversationId === watched.conversationId
      ? { sessionTasks: { ...state.sessionTasks, tasks: applyTasks(state.sessionTasks.tasks, tasks) } }
      : state,
  );
}

/** Folds Brain job updates into the overviews read so far, and task updates into the watched
 * session's tasks (one update each per batch). */
export function applyBrainEvents(envelopes: readonly EventEnvelope[]): void {
  applyTaskEvents(envelopes);
  const jobs = envelopes.flatMap(({ event }) => (event.type === "brainJobUpdated" ? [event.job] : []));
  if (jobs.length === 0) return;
  useBrain.setState((state) => {
    let overviews = state.overviews;
    for (const job of jobs) {
      const overview = overviews[job.projectId];
      if (!overview) continue;
      overviews = { ...overviews, [job.projectId]: { ...overview, jobs: upsertJob(overview.jobs, job) } };
    }
    return overviews === state.overviews ? state : { overviews };
  });
}

export async function queryBrain(key: string, query: BrainQuery): Promise<BrainAnswer> {
  const { answer } = await request({ method: "queryBrain", projectId: projectOf(key), query });
  return answer;
}

export async function getBrainGraph(key: string, filter: NodeFilter): Promise<BrainGraph> {
  const { graph } = await request({ method: "getBrainGraph", projectId: projectOf(key), filter });
  return graph;
}

export async function runBrainJob(projectId: string, kind: BrainJobKind): Promise<string> {
  const { jobId } = await request({ method: "runBrainJob", projectId, kind });
  return jobId;
}

export async function rebuildIndex(projectId: string): Promise<void> {
  await request({ method: "rebuildIndex", projectId });
}

/** The Personal Brain's memories (preferences), newest first. */
export async function listMemories(): Promise<Node[]> {
  const { memories } = await request({ method: "listMemories" });
  return memories;
}

export async function forgetMemory(nodeId: string): Promise<void> {
  await request({ method: "forgetMemory", nodeId });
}

/**
 * "Export conventions to AGENTS.md…": the save panel starts in the project's repository; the
 * outcome (or the error) shows in a toast.
 */
export async function exportProjectConventions(
  projectId: string,
  repo: string | null,
): Promise<void> {
  try {
    const written = await exportConventions(projectId, repo);
    if (!written) return;
    const count = `${written.conventions} ${written.conventions === 1 ? "convention" : "conventions"}`;
    toast(`${written.created ? "Created" : "Updated"} ${written.path} with ${count}`, {
      actions: [{ label: "Show", run: () => void revealPath(written.path).catch(() => {}) }],
    });
  } catch (cause) {
    toast(`Couldn't export conventions: ${cause instanceof Error ? cause.message : String(cause)}`, {
      tone: "error",
    });
  }
}
