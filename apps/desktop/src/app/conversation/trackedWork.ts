import { useEffect, useSyncExternalStore } from "react";
import { request } from "@/ipc/client";
import type { TrackedWork, TrackedPullRequest, WaitingOn } from "@/ipc/generated";

export const EMPTY_TRACKED_WORK: TrackedWork = { pullRequests: [], prHistory: [], waitingOn: [] };
type Snapshot = { work: TrackedWork; loading: boolean; error: string | null };
const EMPTY: Snapshot = { work: EMPTY_TRACKED_WORK, loading: true, error: null };
const entries = new Map<string, { snapshot: Snapshot; listeners: Set<() => void>; pending: boolean }>();
let timer: ReturnType<typeof setInterval> | undefined;
function entry(id: string) {
  let value = entries.get(id);
  if (!value) { value = { snapshot: EMPTY, listeners: new Set(), pending: false }; entries.set(id, value); }
  return value;
}
export async function refreshTrackedWork(id: string) {
  const value = entry(id);
  if (value.pending) return;
  value.pending = true;
  try {
    const { work } = await request({ method: "getTrackedWork", conversationId: id });
    if (!work) throw new Error("Pull request and wait data unavailable");
    value.snapshot = { work, loading: false, error: null };
  } catch (cause) {
    value.snapshot = { ...value.snapshot, loading: false, error: cause instanceof Error ? cause.message : String(cause) };
  } finally { value.pending = false; for (const notify of value.listeners) notify(); }
}
function subscribe(id: string, notify: () => void) {
  const value = entry(id); value.listeners.add(notify);
  if (!timer) timer = setInterval(() => {
    for (const [key, item] of entries) if (item.listeners.size) void refreshTrackedWork(key);
  }, 30_000);
  return () => {
    value.listeners.delete(notify);
    if (![...entries.values()].some((item) => item.listeners.size)) { clearInterval(timer); timer = undefined; }
  };
}
export function useTrackedWork(id: string) {
  const snapshot = useSyncExternalStore((notify) => subscribe(id, notify), () => entry(id).snapshot, () => EMPTY);
  useEffect(() => { void refreshTrackedWork(id); }, [id]);
  return snapshot;
}
export function activePullRequests(work: TrackedWork): TrackedPullRequest[] {
  return work.pullRequests.filter((pr) => !pr.forgotten && pr.pullRequest?.state !== "merged" && pr.pullRequest?.state !== "closed");
}
export function activeWaits(work: TrackedWork): WaitingOn[] { return work.waitingOn.filter((wait) => wait.doneAtMs === null); }
export function waitingDue(wait: WaitingOn, now: number): string {
  if (wait.dueAtMs === null) return `Since ${new Date(wait.createdAtMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
  const date = wait.by?.length === 10 ? wait.by : new Date(wait.dueAtMs).toLocaleDateString([], { timeZone: wait.timezone });
  return `${wait.dueAtMs <= now ? "Overdue" : "By"} ${date}`;
}
export async function completeWait(sessionId: string, id: string) {
  await request({ method: "completeWaitingOn", conversationId: sessionId, id });
  await refreshTrackedWork(sessionId);
}
export async function forgetPr(sessionId: string, key: string) {
  await request({ method: "forgetPullRequest", conversationId: sessionId, key });
  await refreshTrackedWork(sessionId);
}
