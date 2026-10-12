import { create } from "zustand";

import type { ReplaceOutcome, SearchFile, SearchQuery, SearchUpdate } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { CHECKOUTS_KEPT, emitWorkspaceChanged, workspaceKey, type WorkspaceTarget } from "@/state/workspaceCache";

/** What the Search view's fields hold. */
export type SearchForm = {
  text: string;
  replace: string;
  matchCase: boolean;
  wholeWord: boolean;
  regex: boolean;
  include: string;
  exclude: string;
  /** Files to include and exclude shown. */
  details: boolean;
};

export const EMPTY_FORM: SearchForm = {
  text: "", replace: "", matchCase: false, wholeWord: false, regex: false, include: "", exclude: "", details: false,
};

/** What a search found so far, for the checkout it searched. */
export type SearchResults = {
  checkout: string;
  query: SearchQuery;
  files: SearchFile[];
  /** Set once it ended: how many it found, and whether its limits stopped it first. */
  done: { files: number; matches: number; capped: boolean } | null;
  error: string | null;
};

type Entry = { form: SearchForm; results: SearchResults | null };

type SearchState = { entries: Record<string, Entry>; order: readonly string[] };

/** Whose Search: a session, or a draft's project. Its fields stay while the checkout changes. */
export function searchId(target: WorkspaceTarget): string {
  return target.type === "conversation" ? target.conversationId : `project:${target.projectId}`;
}

/** The query a form asks for, or null when there is nothing to search for. */
export function searchQuery(form: SearchForm): SearchQuery | null {
  if (!form.text) return null;
  return { text: form.text, matchCase: form.matchCase, wholeWord: form.wholeWord, regex: form.regex,
    include: form.details ? form.include : "", exclude: form.details ? form.exclude : "" };
}

export function sameQuery(a: SearchQuery | null | undefined, b: SearchQuery | null | undefined): boolean {
  return !!a && !!b && a.text === b.text && a.matchCase === b.matchCase && a.wholeWord === b.wholeWord
    && a.regex === b.regex && a.include === b.include && a.exclude === b.exclude;
}

/**
 * The state after an update of the search with results for `id`: the newest entry, the oldest
 * past the limit gone. Only the few Searches used last keep their results.
 */
export function kept(state: SearchState, id: string, change: Partial<Entry>, limit = CHECKOUTS_KEPT): SearchState {
  const order = [...state.order.filter((other) => other !== id), id];
  const entries = { ...state.entries, [id]: { form: EMPTY_FORM, results: null, ...state.entries[id], ...change } };
  for (const gone of order.splice(0, Math.max(0, order.length - limit))) delete entries[gone];
  return { entries, order };
}

export const useWorkspaceSearch = create<SearchState>()(() => ({ entries: {}, order: [] }));

function patch(id: string, change: Partial<Entry> | ((entry: Entry) => Partial<Entry>)): void {
  useWorkspaceSearch.setState((state) => {
    const entry = state.entries[id] ?? { form: EMPTY_FORM, results: null };
    return kept(state, id, typeof change === "function" ? change(entry) : change);
  });
}

export function setSearchForm(target: WorkspaceTarget, change: Partial<SearchForm>): void {
  patch(searchId(target), (entry) => ({ form: { ...entry.form, ...change } }));
}

/**
 * The search running for this window (the daemon runs one per connection): whose, and its id
 * once the daemon gave it. Until the first update arrives the earlier results stay, so a
 * search run again refreshes them in place.
 */
let running: { token: object; id: number | null; owner: string; fresh: boolean } | null = null;
/** Updates that came before the id their search was given. */
const early = new Map<number, SearchUpdate[]>();

/** Searches a target's checkout, in place of any search running. */
export function runSearch(target: WorkspaceTarget, query: SearchQuery): void {
  const owner = searchId(target);
  const checkout = workspaceKey(target);
  const token = {};
  running = { token, id: null, owner, fresh: true };
  patch(owner, (entry) => ({
    results: entry.results?.checkout === checkout
      ? { ...entry.results, query, done: null, error: null }
      : { checkout, query, files: [], done: null, error: null },
  }));
  request({ method: "startSearch", target, query }).then(({ id }) => {
    if (running?.token !== token) return;
    running.id = id;
    const waiting = early.get(id) ?? [];
    early.clear();
    for (const update of waiting) applyUpdate(id, update);
  }).catch((error: unknown) => {
    if (running?.token !== token) return;
    running = null;
    patch(owner, () => ({
      results: { checkout, query, files: [], done: null, error: error instanceof Error ? error.message : String(error) },
    }));
  });
}

/** Stops a target's search, if it runs; what it found so far stays. */
export function stopSearch(target: WorkspaceTarget): void {
  if (running?.owner !== searchId(target)) return;
  if (running.id !== null) request({ method: "cancelSearch", id: running.id }).catch(() => undefined);
  running = null;
}

/** Stops a target's search and forgets its results: the query was cleared. */
export function clearSearch(target: WorkspaceTarget): void {
  stopSearch(target);
  patch(searchId(target), { results: null });
}

function applyUpdate(id: number, update: SearchUpdate): void {
  if (!running || running.id !== id) return;
  const { owner } = running;
  const fresh = running.fresh;
  running.fresh = false;
  if (update.type === "done") running = null;
  patch(owner, ({ results }) => {
    if (!results) return {};
    const files = fresh ? [] : results.files;
    return update.type === "files"
      ? { results: { ...results, files: [...files, ...update.files] } }
      : { results: { ...results, files, done: { files: update.files, matches: update.matches, capped: update.capped } } };
  });
}

/** Passes an update from the bridge to the search it belongs to. */
export function emitSearchUpdate(id: number, update: SearchUpdate): void {
  if (running && running.id === null) {
    early.set(id, [...(early.get(id) ?? []), update]);
    return;
  }
  applyUpdate(id, update);
}

/**
 * Replaces the matches of a search's results with `replacement` (only `file`'s with one), in
 * the files still as searched. The views then read the checkout again.
 */
export async function replaceResults(target: WorkspaceTarget, results: SearchResults, replacement: string, file?: string): Promise<ReplaceOutcome> {
  const files = results.files.filter((found) => !file || found.path === file)
    .map(({ path, hash, matches }) => ({ path, hash, matches }));
  const { outcome } = await request({ method: "replaceInFiles", target, query: results.query, replacement, files });
  emitWorkspaceChanged(target);
  return outcome;
}
