import { useSyncExternalStore } from "react";

/*
 * Whether a find bar in the chat (app/conversation/ThreadFind.tsx) has a text: while one has, what
 * is folded keeps its hidden parts mounted so they are found (a turn's closed work, a long code
 * block's lines). Its own module, so the thread's parts read it without importing the bar.
 */

/** The find bars open with a text. */
const searching = new Set<object>();
const searchingHeard = new Set<() => void>();

function subscribeSearching(heard: () => void): () => void {
  searchingHeard.add(heard);
  return () => searchingHeard.delete(heard);
}

function findSearching(): boolean {
  return searching.size > 0;
}

/** Whether a find bar searches now: a closed work fold mounts its rows, hidden, so they are found. */
export function useFindSearching(): boolean {
  // The same read serves a server render, as worker history renders on the server in tests.
  return useSyncExternalStore(subscribeSearching, findSearching, findSearching);
}

/** A find bar starts searching; the function returned says it stopped. */
export function startSearching(): () => void {
  const me = {};
  searching.add(me);
  for (const heard of searchingHeard) heard();
  return () => {
    searching.delete(me);
    for (const heard of searchingHeard) heard();
  };
}
