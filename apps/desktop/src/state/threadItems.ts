import { useEffect, useState } from "react";

import { foldTranscript, type TranscriptItem } from "@/components/transcript/transcript";
import { request } from "@/ipc/client";
import type { RawEntry, ThreadItemsPage } from "@/ipc/generated";

/**
 * The thread's own tool calls in full, as its rows open to them (BUILD-PLAN B1b): one request's
 * calls (one turn's, when its rows name it) read with `listThreadItems` and folded the way a
 * worker's transcript is, so a lead's row and a worker's open to the same items. Read only for an
 * opened row, and again only when that row's call moved on (it ended, or printed more), so a
 * running command's Shell box follows its output at most once a second.
 */

/** The calls a page returns. */
const PAGE = 100;
/** The requests' calls kept at once; the oldest read goes first. */
const KEPT = 24;

/** A call's items by its id: its own, and the output a running command printed so far. */
export type CallItems = ReadonlyMap<string, readonly TranscriptItem[]>;

type Entry = {
  /** The newest read, and how each call stood when it started. */
  done: { items: CallItems; stood: ReadonlyMap<string, string> } | null;
  /** A read under way, and how each call stood when it started. */
  going: { promise: Promise<CallItems>; stood: ReadonlyMap<string, string> } | null;
};

const entries = new Map<string, Entry>();

function keyOf(conversationId: string, requestId: string, turn: number | null): string {
  return `${conversationId}|${requestId}|${turn ?? ""}`;
}

/** The item id a folded item's key names: `command:abc` → `abc`. */
function idOf(item: TranscriptItem): string {
  return item.key.slice(item.key.indexOf(":") + 1);
}

/** A request's (or turn's) calls, every page, folded and by call. */
async function readAll(conversationId: string, requestId: string, turn: number | null): Promise<CallItems> {
  const all: RawEntry[] = [];
  let after: number | null = null;
  for (;;) {
    const { page }: { page: ThreadItemsPage } = await request({ method: "listThreadItems", conversationId, requestId, turn, after, limit: PAGE });
    all.push(...page.entries);
    if (page.next === null) break;
    after = page.next;
  }
  const byCall = new Map<string, TranscriptItem[]>();
  for (const item of foldTranscript(all).items) {
    const id = idOf(item);
    byCall.set(id, [...(byCall.get(id) ?? []), item]);
  }
  return byCall;
}

/**
 * The calls of `requestId` (and `turn`) as they stand once `itemId` stood as `stood` says: the
 * last read when it was read so, a read under way that started so, else a new read.
 */
function load(conversationId: string, requestId: string, turn: number | null, itemId: string, stood: string): Promise<CallItems> {
  const key = keyOf(conversationId, requestId, turn);
  let entry = entries.get(key);
  if (!entry) {
    entry = { done: null, going: null };
    entries.set(key, entry);
    // The oldest requests read go first.
    for (const old of entries.keys()) {
      if (entries.size <= KEPT) break;
      if (old !== key) entries.delete(old);
    }
  }
  if (entry.done && entry.done.stood.get(itemId) === stood) return Promise.resolve(entry.done.items);
  if (entry.going && entry.going.stood.get(itemId) === stood) return entry.going.promise;
  const known = new Map([...(entry.done?.stood ?? []), ...(entry.going?.stood ?? [])]);
  known.set(itemId, stood);
  const at = entry;
  const promise = readAll(conversationId, requestId, turn).then(
    (items) => {
      if (at.going?.promise === promise) at.going = null;
      at.done = { items, stood: known };
      return items;
    },
    (error: unknown) => {
      if (at.going?.promise === promise) at.going = null;
      throw error;
    },
  );
  at.going = { promise, stood: known };
  return promise;
}

/**
 * One call's items, read when it is first asked for and again whenever `stood` (how the call
 * stands: its status, its output so far, its end) changes; `null` until read.
 */
export function useCallItems(
  conversationId: string | null,
  requestId: string | null,
  turn: number | null,
  itemId: string,
  stood: string,
): { items: readonly TranscriptItem[] | null; error: string | null } {
  const [state, setState] = useState<{ items: readonly TranscriptItem[] | null; error: string | null; for: string }>({
    items: null,
    error: null,
    for: "",
  });
  const wanted = `${conversationId}|${requestId}|${turn}|${itemId}|${stood}`;
  useEffect(() => {
    if (!conversationId || !requestId) return;
    let current = true;
    load(conversationId, requestId, turn, itemId, stood).then(
      (calls) => current && setState({ items: calls.get(itemId) ?? [], error: null, for: wanted }),
      (cause: unknown) => current && setState((now) => ({ ...now, error: cause instanceof Error ? cause.message : String(cause), for: wanted })),
    );
    return () => {
      current = false;
    };
  }, [conversationId, requestId, turn, itemId, stood, wanted]);
  // A call that moved on keeps what it showed until the new read comes: a streaming Shell box never blanks.
  return { items: state.items, error: state.for === wanted ? state.error : null };
}

/** Forgets every read (tests). */
export function forgetCallItems(): void {
  entries.clear();
}
