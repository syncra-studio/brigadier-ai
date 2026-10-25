import { type ThreadMessage, useAuiState } from "@assistant-ui/react";
import { createContext, useContext, useRef, useSyncExternalStore } from "react";

/*
 * The thread's whole branch, as the app holds it. assistant-ui holds only the messages the list
 * asks for (those it mounts, with some more each way) and the newest turn: it keeps each message
 * it holds live, at a cost on every streamed word, so a long thread held whole would slow every
 * word down (#627). What needs the whole branch (the list's places, the rail, Find, a copy of the
 * thread) reads it here. Without a source (a thread the app doesn't hold, as in fixtures),
 * assistant-ui's messages are the whole branch.
 */

/** The messages the list asks assistant-ui to hold: `from` to `to` (places on the branch) and `ids`. */
export type ThreadHolding = { key: string | null; from: number; to: number; ids: readonly string[] };

export type ThreadListState = {
  /** The conversation they are of (the thread's `scrollKey`). */
  key: string | null;
  /** Every message on the branch shown, oldest first, as assistant-ui sees them. */
  messages: readonly ThreadMessage[];
};

export class ThreadListSource {
  private state: ThreadListState = { key: null, messages: [] };
  private readonly listeners = new Set<() => void>();
  /** Where the list's asks go (the app's view, which feeds assistant-ui). */
  hold: (holding: ThreadHolding) => void = () => {};
  /** What assistant-ui holds now, as the app last fed it. */
  held: ThreadHolding | null = null;

  get = (): ThreadListState => this.state;

  set(key: string | null, messages: readonly ThreadMessage[]): void {
    if (key === this.state.key && messages === this.state.messages) return;
    this.state = { key, messages };
    for (const listener of this.listeners) listener();
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
}

export const ThreadListContext = createContext<ThreadListSource | null>(null);

/** The source of the thread around, if the app holds its branch. */
export function useThreadListSource(): ThreadListSource | null {
  return useContext(ThreadListContext);
}

const noSubscribe = () => () => {};
const NO_MESSAGES: readonly ThreadMessage[] = [];

/**
 * What `select` reads of the thread's whole branch, drawn again when that changes. `select` must
 * give the same value for the same branch (a number, a string, or an array it keeps).
 */
export function useThreadList<T>(select: (state: ThreadListState) => T): T {
  const source = useThreadListSource();
  const held = useAuiState((s) => (source ? NO_MESSAGES : s.thread.messages));
  const fromSource = useSyncExternalStore(source?.subscribe ?? noSubscribe, () => (source ? select(source.get()) : null));
  return source ? (fromSource as T) : select({ key: null, messages: held });
}

/** Each message's place on the branch, made once per branch. */
const placesOf = new WeakMap<readonly ThreadMessage[], Map<string, number>>();

/** Where message `id` is on the branch `messages` (-1: not on it). */
export function placeOf(messages: readonly ThreadMessage[], id: string): number {
  let places = placesOf.get(messages);
  if (!places) {
    places = new Map(messages.map((message, index) => [message.id, index]));
    placesOf.set(messages, places);
  }
  return places.get(id) ?? -1;
}

/** The ids of the branch's messages, the same array while they stay the same. */
export function useThreadListIds(): readonly string[] {
  const last = useRef<readonly string[]>([]);
  return useThreadList((state) => {
    const before = last.current;
    const { messages } = state;
    if (before.length === messages.length && messages.every((message, index) => message.id === before[index])) return before;
    last.current = messages.map((message) => message.id);
    return last.current;
  });
}
