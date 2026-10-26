import { useEffect } from "react";
import { create } from "zustand";

import { request } from "@/ipc/client";
import type { BackgroundHistoryOutput, BackgroundHistoryPage, BackgroundHistoryRecord } from "@/ipc/generated";

export type { BackgroundHistoryRecord, BackgroundHistoryOutput };
type History = { records: BackgroundHistoryRecord[]; total: number | null; next: string | null; loaded: boolean; busy: boolean; error: string | null };
const EMPTY: History = { records: [], total: null, next: null, loaded: false, busy: false, error: null };
export const useBackgroundHistoryStore = create<{ sessions: Record<string, History> }>(() => ({ sessions: {} }));
const revisions = new Map<string, number>();
const message = (error: unknown) => error instanceof Error ? error.message : String(error);
function update(id: string, patch: Partial<History>) {
  useBackgroundHistoryStore.setState(({ sessions }) => ({ sessions: { ...sessions, [id]: { ...(sessions[id] ?? EMPTY), ...patch } } }));
}

export async function loadBackgroundHistory(id: string, more = false): Promise<void> {
  const current = useBackgroundHistoryStore.getState().sessions[id] ?? EMPTY;
  if (current.busy || (more && current.next === null)) return;
  const revision = (revisions.get(id) ?? 0) + 1;
  revisions.set(id, revision);
  update(id, { busy: true, error: null });
  try {
    const { history } = await request({ method: "getBackgroundHistory", conversationId: id, limit: 50, ...(more && current.next ? { before: current.next } : {}) });
    if (revisions.get(id) !== revision) return;
    const keepOlder = more || current.records.length > 50;
    const records = mergeHistory(more ? current.records : history.records, more ? history.records : keepOlder ? current.records : []);
    update(id, { records, total: history.total, next: more || !keepOlder ? history.next : current.next, loaded: true, busy: false });
  } catch (error) {
    if (revisions.get(id) === revision) update(id, { error: message(error), busy: false });
  }
}

/** Later observations replace existing entries; paging never duplicates an event. */
export function mergeHistory(first: readonly BackgroundHistoryRecord[], rest: readonly BackgroundHistoryRecord[]): BackgroundHistoryRecord[] {
  const records = new Map(rest.map((record) => [record.id, record]));
  for (const record of first) records.set(record.id, record);
  return [...records.values()].toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
}

export function useBackgroundHistory(id: string): History {
  const state = useBackgroundHistoryStore((s) => s.sessions[id] ?? EMPTY);
  useEffect(() => {
    void loadBackgroundHistory(id);
    const timer = setInterval(() => void loadBackgroundHistory(id), 5000);
    return () => clearInterval(timer);
  }, [id]);
  return state;
}

/** Summary count reads only one event; lifecycle changes trigger another durable read. */
export function useBackgroundHistoryCount(id: string, changed: string): number | null {
  const total = useBackgroundHistoryStore((s) => s.sessions[id]?.total ?? null);
  useEffect(() => {
    let current = true;
    void changed;
    request({ method: "getBackgroundHistory", conversationId: id, limit: 1 }).then(
      ({ history }: { history: BackgroundHistoryPage }) => { if (current) update(id, { total: history.total }); },
      () => { /* The History tab owns retry/error presentation. */ },
    );
    return () => { current = false; };
  // Lifecycle revision deliberately invalidates the durable count.
  // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [id, changed]);
  return total;
}

export async function readHistoryOutput(id: string, recordId: string): Promise<BackgroundHistoryOutput> {
  return (await request({ method: "readBackgroundHistoryOutput", conversationId: id, recordId })).output;
}

/** Resolve old Output tabs after a CLI/worker went away, without relying on a live task cache. */
export async function findHistoricalOutput(id: string, source: "task" | "preview", taskId: string, worker?: string, startedAtMs?: number): Promise<BackgroundHistoryOutput | null> {
  // Legacy tabs without invocation identity cannot safely address a reused native id.
  if (startedAtMs === undefined) return null;
  let before: string | undefined;
  do {
    const { history } = await request({ method: "getBackgroundHistory", conversationId: id, limit: 100, ...(before && { before }) });
    const record = history.records.find((item) => item.taskId === taskId && (startedAtMs === undefined || item.startedAtMs === startedAtMs) && (source === "preview" ? item.kind === "preview" : item.kind !== "preview" && item.workerId === (worker ?? null)));
    if (record) return readHistoryOutput(id, record.id);
    before = history.next ?? undefined;
  } while (before);
  return null;
}
