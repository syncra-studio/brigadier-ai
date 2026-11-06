import { create } from "zustand";

import { request, RequestError } from "@/ipc/client";
import type { WorkerSource } from "@/ipc/generated";

/**
 * What was last read of each worker's changes, so its row in Source shows them on its first frame:
 * Source's rows are gone whenever another tab of the right sidebar shows. Kept while the app runs.
 */
type Kept = { sources: Record<string, WorkerSource>; order: readonly string[] };

/** How many workers' reads are kept, the ones read longest ago going first. */
export const WORKERS_KEPT = 32;

const useKept = create<Kept>()(() => ({ sources: {}, order: [] }));

const keyOf = (conversationId: string, taskId: string) => JSON.stringify([conversationId, taskId]);

function keep(key: string, source: WorkerSource): void {
  useKept.setState((state) => {
    const order = [...state.order.filter((kept) => kept !== key), key];
    const sources = { ...state.sources, [key]: source };
    for (const old of order.splice(0, Math.max(0, order.length - WORKERS_KEPT))) delete sources[old];
    return { sources, order };
  });
}

function forget(key: string): void {
  useKept.setState((state) => {
    const { [key]: _gone, ...sources } = state.sources;
    return { sources, order: state.order.filter((kept) => kept !== key) };
  });
}

/** What was last read of a worker's changes, until a read says otherwise. */
export function useKeptWorkerSource(conversationId: string, taskId: string): WorkerSource | null {
  return useKept((state) => state.sources[keyOf(conversationId, taskId)] ?? null);
}

/** The read under way for each worker, and what it was begun for. */
const reading = new Map<string, { moved: string; promise: Promise<WorkerSource> }>();
/** The newest read begun of each worker: only it gets kept. */
const newest = new Map<string, number>();
let begun = 0;

/**
 * Reads a worker's changes and keeps them. It shares a read begun for the same `moved` (the
 * worker's state when it was asked) unless asked `again`, as a read the worker moved under, or
 * one the user refreshed, may be out of date. A worker the daemon no longer finds is forgotten.
 */
export function readWorkerSource(conversationId: string, taskId: string, moved: string, again = false): Promise<WorkerSource> {
  const key = keyOf(conversationId, taskId);
  const pending = reading.get(key);
  if (pending && pending.moved === moved && !again) return pending.promise;
  const read = ++begun;
  newest.set(key, read);
  const promise = request({ method: "getWorkerSource", conversationId, taskId }).then(
    ({ source }) => {
      if (newest.get(key) === read) keep(key, source);
      return source;
    },
    (cause: unknown) => {
      if (newest.get(key) === read && cause instanceof RequestError && cause.code === "notFound") forget(key);
      throw cause;
    },
  ).finally(() => {
    if (reading.get(key)?.promise === promise) reading.delete(key);
  });
  reading.set(key, { moved, promise });
  return promise;
}
