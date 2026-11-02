import { useEffect, useMemo } from "react";
import { create } from "zustand";

import { turnCommands } from "@/app/conversation/turnCommands";
import { workingWorkers } from "@/app/conversation/turnWorkers";
import { foldTranscript, type TranscriptItem } from "@/components/conversation/transcript/transcript";
import { request } from "@/ipc/client";
import { type NativeWorker, runningSubagents, subagentPanelOf, subagentRow } from "@/state/nativeWorkers";
import type { AttachmentRef, FileEntry, FileImage, McpServerState, ProviderEvent, ThreadNews, ThreadView, WorkerRow } from "@/ipc/generated";

/**
 * The thread as the CLI keeps it (S9): per conversation, the engine's `threadView` (its CLI's own
 * session files and the running turn's items), read when the thread opens and extended by the
 * live stream's `threadNews`, item by item, the way the engine merges them
 * (`session_file::merge`). A turn's start and end read the view again: only the engine knows its
 * turns, and how each ended.
 */

export const useCliThread = create<{ views: Readonly<Record<string, ThreadView>> }>(() => ({ views: {} }));

/**
 * The conversations whose last read of their thread failed (the daemon's error, or an answer
 * with no view), until one lands: they draw from the messages Brigadier keeps (`keptThread`), not
 * a loading thread that never ends.
 */
export const useUnreadThreads = create<{ failed: Readonly<Record<string, true>> }>(() => ({ failed: {} }));

function readFailed(conversationId: string, failed: boolean): void {
  if (!!useUnreadThreads.getState().failed[conversationId] === failed) return;
  useUnreadThreads.setState((state) => {
    const next: Record<string, true> = { ...state.failed };
    if (failed) next[conversationId] = true;
    else delete next[conversationId];
    return { failed: next };
  });
}

/** The item an event is about, as its CLI named it (`session_file::item_id`). */
export function itemIdOf(event: ProviderEvent): string | null {
  switch (event.type) {
    case "message":
    case "messageDelta":
    case "reasoning":
    case "reasoningDelta":
    case "toolCall":
    case "command":
    case "commandOutputDelta":
    case "fileChanges":
    case "image":
    case "taskNotice":
      return event.itemId;
    default:
      return null;
  }
}

/** What a call's start said that its end leaves out, and the words a delta adds to. */
function keepInput(earlier: ProviderEvent, later: ProviderEvent): ProviderEvent {
  if (earlier.type === "toolCall" && later.type === "toolCall" && later.input === null && earlier.input !== null) {
    return { ...later, input: earlier.input };
  }
  if (earlier.type === "command" && later.type === "command" && later.command === "") return { ...later, command: earlier.command };
  if ((earlier.type === "message" || earlier.type === "messageDelta") && later.type === "messageDelta") {
    return { ...later, text: earlier.text + later.text };
  }
  if ((earlier.type === "reasoning" || earlier.type === "reasoningDelta") && later.type === "reasoningDelta") {
    return { ...later, text: earlier.text + later.text };
  }
  return later;
}

const later = (a: number | undefined, b: number | undefined): number | undefined =>
  a === undefined ? b : b === undefined ? a : Math.max(a, b);

/** Where each item of a merged list sits, by item id (see `mergeEntries`). */
const positions = new WeakMap<readonly FileEntry[], Map<string, number>>();

/**
 * `more` added to `entries`, one entry per item, as the engine merges them: a later event of an
 * item already there takes its place, which stays where the item began; a running command's
 * output grows its row. Entries left as they were keep their identity, so only the rows that
 * changed draw again.
 */
export function mergeEntries(entries: readonly FileEntry[], more: readonly FileEntry[]): FileEntry[] {
  const merged = [...entries];
  // The index of the list merged into last, handed on to the list made from it: a streaming turn
  // merges a delta at a time, and reading every entry again for each would cost the whole thread.
  let at = positions.get(entries);
  if (at) positions.delete(entries);
  else {
    at = new Map<string, number>();
    for (const [index, entry] of merged.entries()) {
      const id = itemIdOf(entry.event);
      if (id !== null) at.set(id, index);
    }
  }
  positions.set(merged, at);
  for (const entry of more) {
    const id = itemIdOf(entry.event);
    const index = id === null ? undefined : at.get(id);
    if (id === null || index === undefined) {
      if (id !== null) at.set(id, merged.length);
      merged.push(entry);
      continue;
    }
    const earlier = merged[index] as FileEntry;
    if (earlier.event.type === "command" && entry.event.type === "commandOutputDelta") {
      merged[index] = {
        ...earlier,
        event: { ...earlier.event, output: (earlier.event.output ?? "") + entry.event.text },
        ...withTime(later(entry.atMs, earlier.updatedAtMs)),
      };
      continue;
    }
    const images = entry.images?.length ? entry.images : earlier.images;
    const atMs = earlier.atMs ?? entry.atMs;
    const parent = entry.parent ?? earlier.parent;
    // Who sent a message and its requests, as the engine noted them: the stream's word of a
    // message the file held first carries them.
    const said = entry.said ?? earlier.said;
    const requests = entry.requests ?? earlier.requests;
    merged[index] = {
      event: keepInput(earlier.event, entry.event),
      ...(atMs !== undefined && { atMs }),
      ...withTime(later(entry.updatedAtMs ?? entry.atMs, earlier.updatedAtMs)),
      ...(parent !== undefined && { parent }),
      ...((entry.steer || earlier.steer) && { steer: true }),
      ...(images?.length && { images }),
      ...(said && { said }),
      ...(requests && { requests }),
    };
  }
  return merged;
}

function withTime(updatedAtMs: number | undefined): { updatedAtMs?: number } {
  return updatedAtMs === undefined ? {} : { updatedAtMs };
}

/** Whether an event only adds to an item: read twice, its words would show twice. */
function isDelta(event: ProviderEvent): boolean {
  return event.type === "messageDelta" || event.type === "reasoningDelta" || event.type === "commandOutputDelta";
}

/** What news carries whole, all of it, when any of it changed: the view's, as it stands. */
type Lists = Partial<Pick<ThreadView, "background" | "mcp" | "workers" | "jobs">>;

/** The lists a piece of news carries. */
function listsOf(news: ThreadNews): Lists {
  return {
    ...(news.background && { background: news.background }),
    ...(news.mcp && { mcp: news.mcp }),
    ...(news.workers && { workers: news.workers }),
    ...(news.jobs && { jobs: news.jobs }),
  };
}

/**
 * How a read and the live stream meet (D5), with no word lost or shown twice. The engine numbers
 * each piece of news (`seq`) under the same lock that changes the live turn a read is made from,
 * and a read says the last number it holds. So a piece of news is in a read, or not, whatever
 * order the two reach the page in:
 * - news keeps drawing while a thread is read, item by item and delta by delta;
 * - a read that lands is drawn with the news heard since it was asked for added, the news it
 *   doesn't hold and only that, in the order heard, deltas included;
 * - news that comes after a read already holding it adds nothing.
 * News with no number (a session's workers' rows) is whole lists only, and always added. A read
 * or news with no number at all (none from the engine) falls back to adding whole items with ids
 * only: a delta, or an item with no id (an error), may be in the read already.
 */
function holds(view: ThreadView, news: ThreadNews): boolean {
  return numbered(view, news) && news.seq! <= view.seq!;
}

function numbered(view: ThreadView, news: ThreadNews): boolean {
  return view.seq !== undefined && news.seq !== undefined;
}

/** `view` with what `news` says that it doesn't hold; `lists`: with the lists it carries too. */
function addNews(view: ThreadView, news: ThreadNews, lists: boolean, read: boolean): ThreadView {
  if (holds(view, news)) return view;
  let entries = news.entries;
  // Unnumbered, only items whole and with ids of their own: one said again only replaces itself.
  if (read && !numbered(view, news)) entries = entries.filter((entry) => !isDelta(entry.event) && itemIdOf(entry.event) !== null);
  const added = lists ? listsOf(news) : {};
  if (entries.length === 0 && Object.keys(added).length === 0) return view;
  return { ...view, ...(entries.length > 0 && { entries: mergeEntries(view.entries, entries) }), ...added };
}

/**
 * Reads under way: the news heard since each was asked for, whether to read once more (a
 * reconnect), and the read itself, which a second ask for the thread joins.
 */
const reads = new Map<string, { heard: ThreadNews[]; again: boolean; done: Promise<void> }>();

/**
 * The news heard from a turn's start or end on, until a read asked for after it: only the engine
 * knows the turns, so their lists (background tasks, MCP servers, workers, jobs) wait for the
 * turns that read brings, never shown before them; its items show as they come. A background
 * command's end clears its hold in the same news that opens the turn it starts, and drawn alone
 * the hold's end read as the block's end: its work folded, then opened again (D21).
 */
const owed = new Map<string, ThreadNews[]>();

function setView(conversationId: string, view: ThreadView): void {
  useCliThread.setState((state) => ({ views: { ...state.views, [conversationId]: view } }));
}

/**
 * Reads a conversation's thread, or joins the read under way: asked twice, it is read once.
 * What the stream said since it was asked for is added once it lands (see `holds`). A turn
 * that started or ended meanwhile reads it once more.
 */
export function loadThreadView(conversationId: string): Promise<void> {
  const going = reads.get(conversationId);
  if (going) return going.done;
  const read = { heard: owed.get(conversationId) ?? [], again: false, done: Promise.resolve() };
  owed.delete(conversationId);
  reads.set(conversationId, read);
  read.done = new Promise((shown) => void readThread(conversationId, read, shown));
  return read.done;
}

/** The turns a thread not held yet shows at once when it opens; the whole of it follows (D46). */
export const NEWEST_TURNS = 5;

/**
 * Reads the thread; `shown` once it can be drawn. A thread not held yet comes in two: its newest
 * turns, drawn at once, then the whole of it, whose older turns fill in above what shows.
 */
async function readThread(conversationId: string, read: { heard: ThreadNews[]; again: boolean }, shown: (value: void) => void): Promise<void> {
  try {
    // Cut, the newest come as just that many turns; with fewer or more they are all there is.
    const newest = useCliThread.getState().views[conversationId] ? null : await threadView(conversationId, NEWEST_TURNS);
    if (newest) {
      land(conversationId, read, newest);
      shown();
    }
    if (!newest || newest.turns.length === NEWEST_TURNS) land(conversationId, read, await threadView(conversationId));
    readFailed(conversationId, false);
  } catch (error) {
    console.error("reading the thread failed", error);
    readFailed(conversationId, true);
  } finally {
    shown();
    reads.delete(conversationId);
    if (read.again || owed.has(conversationId)) void loadThreadView(conversationId);
  }
}

async function threadView(conversationId: string, newestTurns?: number): Promise<ThreadView> {
  const { view } = await request({ method: "threadView", conversationId, ...(newestTurns !== undefined && { newestTurns }) });
  if (!view) throw new Error("the daemon answered with no thread");
  return view;
}

/** A read's view drawn, with what the stream said since it was asked for. */
function land(conversationId: string, read: { heard: ThreadNews[] }, view: ThreadView): void {
  // What waits for the frame is in the read or added to it below: none of it is left to draw
  // again on it.
  pending.delete(conversationId);
  draining.delete(conversationId);
  // News from a turn's start or end this read wasn't asked after: its lists wait for the next.
  const afterTurn = new Set(owed.get(conversationId));
  let landed = view;
  for (const news of read.heard) landed = addNews(landed, news, !afterTurn.has(news), true);
  setView(conversationId, landed);
}

/**
 * The frame queue (spec 3.1): what the stream said, drawn at the next frame, once for all
 * sessions, not once per delta (each its own task in the WebView). Per conversation, in order:
 * - an answer's words come out at about 24 characters a frame (`round(24 × min(elapsed, 500) /
 *   16.67)` per item), so a big chunk flows instead of popping in; reasoning comes out whole;
 * - a command's output gathers and comes out every 50 ms, its last 20,000 characters;
 * - anything else (an item begun or ended, a turn's start or end, the lists) waits for the words
 *   before it: with 24 or fewer left they come out at once, else faster, so they are out within
 *   8 frames, then it is drawn. Words never trail what comes after them.
 * A hidden window draws no frames: everything comes out at once (so do the unit tests, with no page).
 */
type Words = { kind: "text" | "output"; news: ThreadNews; entry: FileEntry; text: string };
type Op = Words | { kind: "structure"; news: ThreadNews; entries: FileEntry[]; envelope: boolean };

const pending = new Map<string, Op[]>();
/** Per conversation, the frames left to bring out the words before what waits on them. */
const draining = new Map<string, number>();
let frame = 0;
let timer: ReturnType<typeof setTimeout> | null = null;
let lastFrameAt: number | null = null;
let lastOutputAt = 0;
const FRAME_MS = 1000 / 60;
const CHARS_PER_FRAME = 24;
const DRAIN_FRAMES = 8;
const OUTPUT_EVERY_MS = 50;
const OUTPUT_KEPT = 20_000;
const NEWS_WAIT_MS = 100;

/** Whether the app draws frames: outside a page (the unit tests) everything goes on at once. */
const inPage = () => typeof document !== "undefined" && typeof requestAnimationFrame === "function";

function scheduleNews(): void {
  if (frame || timer !== null) return;
  if (inPage() && !document.hidden) frame = requestAnimationFrame(flushNews);
  // A frame that doesn't come (a window hidden meanwhile) doesn't hold the news back.
  timer = setTimeout(flushNews, inPage() ? NEWS_WAIT_MS : 0);
}

/** `news`, as the queue holds it: its words, a command's output, and the rest of it. */
function opsOf(news: ThreadNews): Op[] {
  const ops: Op[] = news.entries.map((entry) => {
    const { event } = entry;
    if (event.type === "messageDelta" || event.type === "reasoningDelta") return { kind: "text", news, entry, text: event.text };
    if (event.type === "commandOutputDelta") return { kind: "output", news, entry, text: event.text };
    return { kind: "structure", news, entries: [entry], envelope: false };
  });
  if (news.opened || news.ended || Object.keys(listsOf(news)).length > 0) ops.push({ kind: "structure", news, entries: [], envelope: true });
  return ops;
}

/** `entry`, its delta's text `text`. */
function saying(entry: FileEntry, text: string): FileEntry {
  return { ...entry, event: { ...entry.event, text } as ProviderEvent };
}

/**
 * Brings out what conversation `id`'s queue `ops` can this frame, by `draw`: `normal` characters
 * of each item's words, or all of it (`all`). Gives what is left.
 */
function release(id: string, ops: Op[], draw: (news: ThreadNews, entries: FileEntry[], envelope: boolean) => void, normal: number, all: boolean, outputDue: boolean): Op[] {
  const budget = new Map<string, number>();
  let at = 0;
  while (at < ops.length) {
    const wall = ops.findIndex((op, index) => index >= at && op.kind === "structure");
    const end = wall === -1 ? ops.length : wall;
    const run = ops.slice(at, end) as Words[];
    const paced = (op: Op) => op.kind === "text" && op.entry.event.type === "messageDelta";
    const left = new Map<string, number>();
    for (const op of run) if (paced(op)) left.set(itemIdOf(op.entry.event)!, (left.get(itemIdOf(op.entry.event)!) ?? 0) + op.text.length);
    const total = [...left.values()].reduce((sum, count) => sum + count, 0);
    const whole = all || (wall !== -1 && total <= CHARS_PER_FRAME);
    // Something waits on more than a frame's words: they come out within its frames left.
    const frames = !whole && wall !== -1 ? (draining.get(id) ?? DRAIN_FRAMES) : 0;
    if (frames) draining.set(id, Math.max(frames - 1, 1));
    for (const op of run) {
      if (op.kind !== "text") continue;
      const key = itemIdOf(op.entry.event)!;
      let count = op.text.length;
      if (!whole && paced(op)) {
        const allowed = budget.get(key) ?? (frames ? Math.max(normal, Math.ceil(left.get(key)! / frames)) : normal);
        count = Math.min(count, allowed);
        // Never half a surrogate pair.
        if (count > 0 && count < op.text.length && /[\uD800-\uDBFF]/.test(op.text[count - 1]!)) count += 1;
        budget.set(key, Math.max(allowed - count, 0));
      }
      if (count > 0) draw(op.news, [saying(op.entry, op.text.slice(0, count))], false);
      op.text = op.text.slice(count);
    }
    const waiting = run.some((op) => op.kind === "text" && op.text.length > 0);
    if (whole || outputDue || (wall !== -1 && !waiting)) {
      // Each command's output since the last, as one delta: its last 20,000 characters.
      const outputs = new Map<string, Words>();
      for (const op of run) {
        if (op.kind !== "output") continue;
        const key = itemIdOf(op.entry.event)!;
        const held = outputs.get(key);
        outputs.set(key, held ? { ...op, text: (held.text + op.text).slice(-OUTPUT_KEPT) } : { ...op, text: op.text.slice(-OUTPUT_KEPT) });
        op.text = "";
      }
      for (const op of outputs.values()) draw(op.news, [saying(op.entry, op.text)], false);
    }
    if (wall === -1 || waiting) return [...run.filter((op) => op.text.length > 0), ...ops.slice(end)];
    const structure = ops[wall] as Extract<Op, { kind: "structure" }>;
    draw(structure.news, structure.entries, structure.envelope);
    draining.delete(id);
    at = wall + 1;
  }
  return [];
}

function flushNews(time?: number): void {
  if (frame) cancelAnimationFrame(frame);
  if (timer !== null) clearTimeout(timer);
  frame = 0;
  timer = null;
  const now = typeof time === "number" ? time : performance.now();
  const all = !inPage() || document.hidden;
  const elapsed = lastFrameAt === null ? FRAME_MS : Math.min(Math.max(now - lastFrameAt, 0), 500);
  lastFrameAt = now;
  const normal = Math.max(1, Math.round((CHARS_PER_FRAME * elapsed) / FRAME_MS));
  const outputDue = now - lastOutputAt >= OUTPUT_EVERY_MS;
  if (outputDue) lastOutputAt = now;
  const { views } = useCliThread.getState();
  const next: Record<string, ThreadView> = {};
  const turned = new Set<string>();
  for (const [id, ops] of pending) {
    let view: ThreadView | undefined = views[id];
    // A thread not read yet is read whole when it opens.
    if (!view) {
      pending.delete(id);
      draining.delete(id);
      continue;
    }
    const draw = (news: ThreadNews, entries: FileEntry[], envelope: boolean) => {
      // A turn's start or end reads the thread again, once any read under way lands; its lists
      // wait for that read.
      if (envelope && (news.opened || news.ended) && owed.has(id)) turned.add(id);
      view = addNews(view!, { ...news, entries }, envelope && !owed.has(id), false);
    };
    const left = release(id, ops, draw, normal, all, outputDue);
    if (left.length > 0) pending.set(id, left);
    else pending.delete(id);
    if (view !== views[id]) next[id] = view;
  }
  if (Object.keys(next).length > 0) useCliThread.setState((state) => ({ views: { ...state.views, ...next } }));
  for (const id of turned) if (!reads.has(id)) void loadThreadView(id);
  if (pending.size > 0) scheduleNews();
  else lastFrameAt = null;
}

/** What a CLI's live stream added to a conversation's thread (`threadNews`). */
export function applyThreadNews(news: ThreadNews): void {
  const id = news.conversationId;
  const view = useCliThread.getState().views[id];
  // Already in the thread as read (the read came first).
  if (view && holds(view, news)) return;
  const read = reads.get(id);
  read?.heard.push(news);
  if ((news.opened || news.ended) && (read || view) && !owed.has(id)) owed.set(id, []);
  owed.get(id)?.push(news);
  if (!view) return;
  const ops = opsOf(news);
  if (ops.length === 0) return;
  pending.set(id, [...(pending.get(id) ?? []), ...ops]);
  scheduleNews();
}

/** When the app started: a conversation's view not read since was never read here. */
const APP_START_MS = Date.now();

/** When each conversation's view was dropped (`resyncThreadViews`). */
const dropped = new Map<string, number>();

/**
 * Since when conversation `conversationId` has had no view read: its turns were all before that,
 * so only a message stored since may be on its way while its view is read (`cliThreadTree`).
 */
export function unreadSince(conversationId: string): number {
  return dropped.get(conversationId) ?? APP_START_MS;
}

/**
 * After (re)connecting: the news missed is read again with the shown conversations' threads;
 * the others are dropped and read when they open.
 */
export function resyncThreadViews(shown: readonly string[]): Promise<void> {
  const { views } = useCliThread.getState();
  const now = Date.now();
  for (const id of Object.keys(views)) if (!shown.includes(id)) dropped.set(id, now);
  useCliThread.setState({ views: Object.fromEntries(shown.flatMap((id) => (views[id] ? [[id, views[id]] as const] : []))) });
  // A read asked for before the reconnect may predate what was missed: it reads once more.
  for (const read of reads.values()) read.again = true;
  return Promise.all(shown.map((id) => loadThreadView(id))).then(() => {});
}

/** Whether conversation `conversationId`'s thread has no view because its read failed. */
export function useThreadUnreadable(conversationId: string | null): boolean {
  return useUnreadThreads((s) => (conversationId ? !!s.failed[conversationId] : false));
}

/** A conversation's thread, read when first asked for; `null` until read. */
export function useThreadView(conversationId: string | null): ThreadView | null {
  const view = useCliThread((s) => (conversationId ? (s.views[conversationId] ?? null) : null));
  useEffect(() => {
    if (conversationId && !useCliThread.getState().views[conversationId]) void loadThreadView(conversationId);
  }, [conversationId]);
  return view;
}

const NO_FILE_ENTRIES: readonly FileEntry[] = [];
const facts = new WeakMap<(entries: readonly FileEntry[]) => unknown, WeakMap<readonly FileEntry[], unknown>>();

/**
 * What `fact` reads from conversation `conversationId`'s CLI items, worked out once per change of
 * them. A component reading a fact this way draws again only when the fact changes, not on every
 * streamed word: `fact` should give a string or a number.
 */
export function useThreadFact<T extends string | number | boolean>(conversationId: string | null, fact: (entries: readonly FileEntry[]) => T): T {
  return useCliThread((s) => {
    const entries = (conversationId ? s.views[conversationId]?.entries : undefined) ?? NO_FILE_ENTRIES;
    let known = facts.get(fact);
    if (!known) {
      known = new WeakMap();
      facts.set(fact, known);
    }
    if (known.has(entries)) return known.get(entries) as T;
    const value = fact(entries);
    known.set(entries, value);
    return value;
  });
}

/**
 * When a conversation's newest turn started, while it runs; `null` once it ended or until its
 * thread is read. Only a turn's start or end draws its readers again, not each streamed word.
 */
export function useOpenTurnStart(conversationId: string | null): number | null {
  return useCliThread((s) => {
    const last = conversationId ? s.views[conversationId]?.turns.at(-1) : undefined;
    return last && last.end === null ? last.startedAtMs : null;
  });
}

const NO_ROWS: readonly WorkerRow[] = [];
const NO_SERVERS: readonly McpServerState[] = [];

/** A session's own workers, as their CLIs and git show them (S14), newest first: never its CLI's own subagents (D96). */
export function useWorkerRows(conversationId: string | null): readonly WorkerRow[] {
  return useCliThread((s) => (conversationId ? (s.views[conversationId]?.workers ?? NO_ROWS) : NO_ROWS));
}

/**
 * The subagent panel `panel` of session `conversationId` shows (`subagentPanel`): one of the
 * session's own, or of one of its workers, from that thread as last read; else `null`.
 */
export function useSubagentRow(conversationId: string | null, panel: string | null | undefined): NativeWorker | null {
  const lead = useCliThread((s) => {
    const of = subagentPanelOf(panel);
    const view = conversationId && of?.owner === conversationId ? s.views[conversationId] : undefined;
    return view && of ? subagentRow(conversationId!, of.owner, view, of.callId) : null;
  });
  const worker = useWorkerThreads((s) => {
    const of = subagentPanelOf(panel);
    const view = conversationId && of && of.owner !== conversationId ? s.views[of.owner] : undefined;
    return view && of ? subagentRow(conversationId!, of.owner, view, of.callId) : null;
  });
  return lead ?? worker;
}

/** How many of its CLI's own subagents a session's block that ran from `startedAtMs` to `endedAtMs` started still work (`runningSubagents`). */
export function useTurnSubagents(conversationId: string | null, startedAtMs: number, endedAtMs: number | null): number {
  return useCliThread((s) => {
    const view = conversationId ? s.views[conversationId] : undefined;
    return view ? runningSubagents(view, startedAtMs, endedAtMs) : 0;
  });
}

/** How many of the workers a session's block, its turns started at `starts`, started still work (`workingWorkers`). */
export function useTurnWorkers(conversationId: string | null, starts: readonly number[], endedAtMs: number | null = null): number {
  return useCliThread((s) => {
    const view = conversationId && starts.length > 0 ? s.views[conversationId] : undefined;
    return view ? workingWorkers(view.workers ?? NO_ROWS, view.turns, starts, endedAtMs, !!view.jobs?.waking) : 0;
  });
}

/** How many of the background commands a session's block, its turns started at `starts`, started still hold it (`turnCommands`, D21). */
export function useTurnCommands(conversationId: string | null, starts: readonly number[], endedAtMs: number | null = null): number {
  return useCliThread((s) => {
    const view = conversationId && starts.length > 0 ? s.views[conversationId] : undefined;
    return view ? turnCommands(view, view.turns, starts, endedAtMs) : 0;
  });
}

/** The MCP servers a session's own CLI has, as it last said. */
export function useSessionMcp(conversationId: string): readonly McpServerState[] {
  return useCliThread((s) => s.views[conversationId]?.mcp ?? NO_SERVERS);
}

/** A worker's row, in whichever session's thread lists it. */
export function workerRowIn(views: Readonly<Record<string, ThreadView>>, taskId: string): { conversationId: string; row: WorkerRow } | null {
  for (const [conversationId, view] of Object.entries(views)) {
    const row = view.workers?.find((each) => each.id === taskId);
    if (row) return { conversationId, row };
  }
  return null;
}

/** A worker's row, live; `null` while no thread read lists it. */
export function useWorkerRow(taskId: string | null): WorkerRow | null {
  return useCliThread((s) => (taskId ? (workerRowIn(s.views, taskId)?.row ?? null) : null));
}

/**
 * Workers' own threads, read only (`workerThread`): each from its CLI's session files, by task
 * id, read when its panel opens and again as its row changes.
 */
export const useWorkerThreads = create<{ views: Readonly<Record<string, ThreadView>> }>(() => ({ views: {} }));

/** The latest read asked for of each worker's thread. */
const workerReads = new Map<string, number>();

/** Reads worker `taskId`'s thread into `useWorkerThreads`. */
export async function loadWorkerThread(conversationId: string, taskId: string): Promise<void> {
  const read = (workerReads.get(taskId) ?? 0) + 1;
  workerReads.set(taskId, read);
  const { view } = await request({ method: "workerThread", conversationId, taskId });
  // An older read answering late never replaces a newer one.
  if (workerReads.get(taskId) !== read) return;
  useWorkerThreads.setState((state) => ({ views: { ...state.views, [taskId]: view } }));
}

/** A view's entries by item id. */
const indexes = new WeakMap<readonly FileEntry[], ReadonlyMap<string, FileEntry>>();

function entryIn(view: ThreadView | undefined, itemId: string): FileEntry | undefined {
  if (!view) return undefined;
  let index = indexes.get(view.entries);
  if (!index) {
    const byId = new Map<string, FileEntry>();
    for (const entry of view.entries) {
      const id = itemIdOf(entry.event);
      if (id !== null) byId.set(id, entry);
    }
    index = byId;
    indexes.set(view.entries, index);
  }
  return index.get(itemId);
}

/** An item of a conversation's thread, as it stands now. */
export function cliEntry(conversationId: string, itemId: string): FileEntry | undefined {
  return entryIn(useCliThread.getState().views[conversationId], itemId);
}

/**
 * A call's items, as a row opens to them, folded from the call's own entry the way a worker's
 * transcript is; `null` with `conversationId` null. The entry follows the stream, so a running
 * command's Shell box grows as its output comes.
 */
export function useCliCallItems(conversationId: string | null, itemId: string): { items: readonly TranscriptItem[]; error: null } | null {
  const entry = useCliThread((s) => (conversationId ? entryIn(s.views[conversationId], itemId) : undefined));
  return useMemo(() => {
    if (conversationId === null) return null;
    const items = entry ? foldTranscript([{ streamSeq: 0, atMs: entry.atMs ?? 0, event: entry.event }]).items : [];
    return { items, error: null };
  }, [conversationId, entry]);
}

/** An image a session file holds, read from it (`readSessionFileImage`), as a data URL. */
export async function readSessionImage(conversationId: string, image: FileImage): Promise<string> {
  const { data, mime } = await request({ method: "readSessionFileImage", conversationId, image });
  return `data:${mime};base64,${data}`;
}

/** What marks an attachment that is an image in the CLI's session file, not in the blob store. */
const IMAGE_REF = "session-file-image:";

/** A user message's image in the session file, as its bubble's attachment. */
export function sessionImageRef(image: FileImage): AttachmentRef {
  return { id: `${IMAGE_REF}${JSON.stringify(image)}`, name: "Image", mime: image.mime, bytes: 0, pasted: false, inline: null };
}

/** The session file image an attachment is, if it is one. */
export function sessionImageOf(ref: AttachmentRef): FileImage | null {
  return ref.id.startsWith(IMAGE_REF) ? (JSON.parse(ref.id.slice(IMAGE_REF.length)) as FileImage) : null;
}

/** A session file image's bytes, as an attachment's reader gives them. */
export async function readSessionImageBlob(conversationId: string, image: FileImage): Promise<Blob> {
  const { data, mime } = await request({ method: "readSessionFileImage", conversationId, image });
  const binary = atob(data);
  return new Blob([Uint8Array.from(binary, (char) => char.charCodeAt(0))], { type: mime });
}
