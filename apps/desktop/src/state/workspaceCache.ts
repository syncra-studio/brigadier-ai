import { create } from "zustand";

import type { Conversation, Project, SourceState } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { useApp } from "@/state/store";

/** Whose files the right sidebar shows: a session's checkout, or a draft's project folder. */
export type WorkspaceTarget =
  | { type: "conversation"; conversationId: string }
  | { type: "project"; projectId: string };

export type FileList = { files: string[]; truncated: boolean };

/** What was last read of one checkout, so the right sidebar's first frame shows it. */
export type WorkspaceEntry = { files?: FileList; source?: SourceState };

type CacheState = { entries: Record<string, WorkspaceEntry>; order: readonly string[] };

/** How many checkouts are kept: the open one and the few visited just before it. */
export const CHECKOUTS_KEPT = 4;
/** Total retained payload budget, measured as UTF-16 JSON bytes. */
export const CACHE_BYTES = 8 * 1024 * 1024;

/**
 * Which checkout a target reads, as a key. A session that moves from its repo to its own copy,
 * or loses and gets back that copy, reads another checkout and so gets another key.
 */
export function checkoutKey(target: WorkspaceTarget, conversations: Record<string, Conversation>, projects: Record<string, Project>): string {
  if (target.type === "project") {
    return JSON.stringify(["project", target.projectId, projects[target.projectId]?.repos.map((repo) => repo.path)]);
  }
  const conversation = conversations[target.conversationId];
  const environment = conversation?.setup?.type === "session" ? conversation.setup.environment : null;
  const checkout = environment?.type === "newWorktree" ? environment.path ?? "repo" : "local";
  return JSON.stringify(["session", target.conversationId, conversation?.setup?.type === "session" ? conversation.setup.repo : null, checkout, environment?.branch, conversation?.lifecycle]);
}

/** The cache after `patch` lands on `key`: it becomes the newest, and the oldest past the limit go. */
export function remembered(state: CacheState, key: string, patch: WorkspaceEntry, limit = CHECKOUTS_KEPT, byteLimit = CACHE_BYTES): CacheState {
  const order = [...state.order.filter((kept) => kept !== key), key];
  const dropped = order.splice(0, Math.max(0, order.length - limit));
  const entries = { ...state.entries, [key]: { ...state.entries[key], ...patch } };
  for (const gone of dropped) delete entries[gone];
  let bytes = order.reduce((sum, kept) => sum + JSON.stringify(entries[kept]).length * 2, 0);
  while (bytes > byteLimit && order.length) {
    const gone = order.shift()!;
    bytes -= JSON.stringify(entries[gone]).length * 2;
    delete entries[gone];
  }
  return { entries, order };
}

const useWorkspaceCache = create<CacheState>()(() => ({ entries: {}, order: [] }));

function remember(key: string, patch: WorkspaceEntry): void {
  useWorkspaceCache.setState((state) => remembered(state, key, patch));
}

// Generations follow object replacements with the same identity, but never a checkout that
// went away and came back at the same path. Weak keys don't retain deleted catalog records.
const generations = new WeakMap<Conversation | Project, number>();
let generation = 0;

export function workspaceKey(target: WorkspaceTarget): string {
  const { conversations, projects } = useApp.getState();
  const record = target.type === "conversation" ? conversations[target.conversationId] : projects[target.projectId];
  if (record && !generations.has(record)) generations.set(record, ++generation);
  return `${checkoutKey(target, conversations, projects)}:${record ? generations.get(record) : "missing"}`;
}

useApp.subscribe((next, previous) => {
  if (next.conversations === previous.conversations && next.projects === previous.projects) return;
  const targets: WorkspaceTarget[] = [
    ...Object.keys(previous.conversations).map((conversationId) => ({ type: "conversation", conversationId }) as const),
    ...Object.keys(previous.projects).map((projectId) => ({ type: "project", projectId }) as const),
  ];
  const invalid = new Set<string>();
  for (const target of targets) {
    const old = target.type === "conversation" ? previous.conversations[target.conversationId] : previous.projects[target.projectId];
    const current = target.type === "conversation" ? next.conversations[target.conversationId] : next.projects[target.projectId];
    const stamp = old && generations.get(old);
    if (!stamp || old === current) continue;
    const identity = checkoutKey(target, previous.conversations, previous.projects);
    if (current && identity === checkoutKey(target, next.conversations, next.projects)) generations.set(current, stamp);
    else invalid.add(`${identity}:${stamp}`);
  }
  if (invalid.size) useWorkspaceCache.setState((state) => ({
    entries: Object.fromEntries(Object.entries(state.entries).filter(([key]) => !invalid.has(key))),
    order: state.order.filter((key) => !invalid.has(key)),
  }));
});

/** The key changes on checkout transitions, even when the same path is used again. */
export function useCheckoutKey(target: WorkspaceTarget): string {
  return useApp(() => workspaceKey(target));
}

/** What can be reused when a target opens; oversized payloads stay only in the mounted view. */
export function cachedWorkspace(target: WorkspaceTarget): WorkspaceEntry | undefined {
  return useWorkspaceCache.getState().entries[workspaceKey(target)];
}

export function useCachedEntry(key: string): WorkspaceEntry | undefined {
  return useWorkspaceCache((s) => s.entries[key]);
}

/** Reads already on their way, so a prefetch and a view opening share one. */
const reading = new Map<string, Promise<unknown>>();

function once<T>(id: string, read: () => Promise<T>): Promise<T> {
  const pending = reading.get(id) as Promise<T> | undefined;
  if (pending) return pending;
  const next = read().finally(() => reading.delete(id));
  reading.set(id, next);
  return next;
}

function sameFiles(a: FileList | undefined, b: FileList): boolean {
  return !!a && a.truncated === b.truncated && a.files.length === b.files.length && a.files.every((file, index) => file === b.files[index]);
}

/** Reads a target's files and keeps them; an unchanged list keeps its old identity. */
export function readFiles(target: WorkspaceTarget): Promise<FileList> {
  const key = workspaceKey(target);
  return once(`files:${key}`, async () => {
    const { files, truncated } = target.type === "conversation"
      ? await request({ method: "listFiles", conversationId: target.conversationId, query: null })
      : await request({ method: "listProjectFiles", projectId: target.projectId, query: null });
    const list = { files, truncated };
    const kept = useWorkspaceCache.getState().entries[key]?.files;
    if (sameFiles(kept, list)) return kept!;
    if (workspaceKey(target) === key) remember(key, { files: list });
    return list;
  });
}

/** Reads a target's source state and keeps it. */
export function readSource(target: WorkspaceTarget): Promise<SourceState> {
  const key = workspaceKey(target);
  return once(`source:${key}`, async () => {
    const { state } = target.type === "conversation"
      ? await request({ method: "getSourceState", conversationId: target.conversationId })
      : await request({ method: "getProjectSourceState", projectId: target.projectId });
    if (workspaceKey(target) === key) remember(key, { source: state });
    return state;
  });
}

/** Keeps what a change or commit returned, so reopening shows it. */
export function keepSource(target: WorkspaceTarget, state: SourceState, key = workspaceKey(target)): void {
  if (workspaceKey(target) === key) remember(key, { source: state });
}

/**
 * Reads what the right sidebar would show for a target and isn't kept yet, so that opening it
 * paints the contents at once. Failures are left to the views, which read again.
 */
export function prefetchWorkspace(target: WorkspaceTarget): void {
  const entry = cachedWorkspace(target);
  if (entry) remember(workspaceKey(target), {});
  if (!entry?.files) readFiles(target).catch(() => undefined);
  if (!entry?.source) readSource(target).catch(() => undefined);
}
