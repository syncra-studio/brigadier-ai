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

/**
 * Which checkout a target reads, as a key. A session that moves from its repo to its own copy,
 * or loses and gets back that copy, reads another checkout and so gets another key.
 */
export function checkoutKey(target: WorkspaceTarget, conversations: Record<string, Conversation>, projects: Record<string, Project>): string {
  if (target.type === "project") {
    const repos = projects[target.projectId]?.repos.map((repo) => repo.path).join("|") ?? "";
    return `project:${target.projectId}:${repos}`;
  }
  const conversation = conversations[target.conversationId];
  const environment = conversation?.setup?.type === "session" ? conversation.setup.environment : null;
  const checkout = environment?.type === "newWorktree" ? environment.path ?? "repo" : "local";
  return `session:${target.conversationId}:${checkout}:${conversation?.lifecycle ?? ""}`;
}

/** The cache after `patch` lands on `key`: it becomes the newest, and the oldest past the limit go. */
export function remembered(state: CacheState, key: string, patch: WorkspaceEntry, limit = CHECKOUTS_KEPT): CacheState {
  const order = [...state.order.filter((kept) => kept !== key), key];
  const dropped = order.splice(0, Math.max(0, order.length - limit));
  const entries = { ...state.entries, [key]: { ...state.entries[key], ...patch } };
  for (const gone of dropped) delete entries[gone];
  return { entries, order };
}

const useWorkspaceCache = create<CacheState>()(() => ({ entries: {}, order: [] }));

function remember(key: string, patch: WorkspaceEntry): void {
  useWorkspaceCache.setState((state) => remembered(state, key, patch));
}

function keyOf(target: WorkspaceTarget): string {
  const { conversations, projects } = useApp.getState();
  return checkoutKey(target, conversations, projects);
}

/** The key of the checkout a target reads now; it changes when the session moves checkout. */
export function useCheckoutKey(target: WorkspaceTarget): string {
  return useApp((s) => checkoutKey(target, s.conversations, s.projects));
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
  const key = keyOf(target);
  return once(`files:${key}`, async () => {
    const { files, truncated } = target.type === "conversation"
      ? await request({ method: "listFiles", conversationId: target.conversationId, query: null })
      : await request({ method: "listProjectFiles", projectId: target.projectId, query: null });
    const list = { files, truncated };
    const kept = useWorkspaceCache.getState().entries[key]?.files;
    if (sameFiles(kept, list)) return kept!;
    remember(key, { files: list });
    return list;
  });
}

/** Reads a target's source state and keeps it. */
export function readSource(target: WorkspaceTarget): Promise<SourceState> {
  const key = keyOf(target);
  return once(`source:${key}`, async () => {
    const { state } = target.type === "conversation"
      ? await request({ method: "getSourceState", conversationId: target.conversationId })
      : await request({ method: "getProjectSourceState", projectId: target.projectId });
    remember(key, { source: state });
    return state;
  });
}

/** Keeps what a change or commit returned, so reopening shows it. */
export function keepSource(target: WorkspaceTarget, state: SourceState): void {
  remember(keyOf(target), { source: state });
}

/**
 * Reads what the right sidebar would show for a target and isn't kept yet, so that opening it
 * paints the contents at once. Failures are left to the views, which read again.
 */
export function prefetchWorkspace(target: WorkspaceTarget): void {
  const entry = useWorkspaceCache.getState().entries[keyOf(target)];
  if (!entry?.files) readFiles(target).catch(() => undefined);
  if (!entry?.source) readSource(target).catch(() => undefined);
}
