import { useEffect, useMemo } from "react";
import { create } from "zustand";

import { foldTranscript, type TranscriptItem } from "@/components/conversation/transcript/transcript";
import { request } from "@/ipc/client";
import { workersOf } from "@/state/nativeWorkers";
import type { AttachmentRef, FileEntry, FileImage, McpServerState, ProviderEvent, ThreadNews, ThreadView, WorkerRow } from "@/ipc/generated";

/**
 * The thread as the CLI keeps it (S9): per conversation, the engine's `threadView` (its CLI's own
 * session files and the running turn's items), read when the thread opens and extended by the
 * live stream's `threadNews`, item by item, the way the engine merges them
 * (`session_file::merge`). A turn's start and end read the view again: only the engine knows its
 * turns, and how each ended.
 */

export const useCliThread = create<{ views: Readonly<Record<string, ThreadView>> }>(() => ({ views: {} }));

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
    merged[index] = {
      event: keepInput(earlier.event, entry.event),
      ...(atMs !== undefined && { atMs }),
      ...withTime(later(entry.updatedAtMs ?? entry.atMs, earlier.updatedAtMs)),
      ...(parent !== undefined && { parent }),
      ...((entry.steer || earlier.steer) && { steer: true }),
      ...(images?.length && { images }),
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
type Lists = Partial<Pick<ThreadView, "background" | "mcp" | "workers">>;

/** The lists a piece of news carries. */
function listsOf(news: ThreadNews): Lists {
  return {
    ...(news.background && { background: news.background }),
    ...(news.mcp && { mcp: news.mcp }),
    ...(news.workers && { workers: news.workers }),
  };
}

/**
 * Reads under way, with what the stream said meanwhile (items, and the CLI's background tasks,
 * MCP servers and the session's workers when they changed), and whether a turn ended meanwhile.
 */
const reads = new Map<string, { heard: FileEntry[]; lists: Lists; again: boolean }>();

function setView(conversationId: string, view: ThreadView): void {
  useCliThread.setState((state) => ({ views: { ...state.views, [conversationId]: view } }));
}

/**
 * Reads a conversation's thread. What the stream said while it was read is added once it lands:
 * whole items again (they only replace themselves), not deltas, which the view may hold already
 * (their item's own end brings its whole text). News still waiting for the frame when it lands
 * was said while it was read, too. A turn that ended meanwhile reads it once more.
 */
export async function loadThreadView(conversationId: string): Promise<void> {
  const going = reads.get(conversationId);
  if (going) {
    going.again = true;
    return;
  }
  const read: { heard: FileEntry[]; lists: Lists; again: boolean } = { heard: [], lists: {}, again: false };
  reads.set(conversationId, read);
  try {
    const { view } = await request({ method: "threadView", conversationId });
    // What waits for the frame goes to the read now, while it is still under way.
    if (queued.length > 0) flushNews();
    const heard = read.heard.filter((entry) => !isDelta(entry.event));
    // One without an id of its own (an error) is the view's already when it holds the same.
    const seen = new Set(view.entries.filter((entry) => itemIdOf(entry.event) === null).map((entry) => JSON.stringify(entry)));
    const fresh = heard.filter((entry) => itemIdOf(entry.event) !== null || !seen.has(JSON.stringify(entry)));
    const landed = fresh.length > 0 ? { ...view, entries: mergeEntries(view.entries, fresh) } : view;
    setView(conversationId, { ...landed, ...read.lists });
  } catch (error) {
    console.error("reading the thread failed", error);
  } finally {
    reads.delete(conversationId);
    if (read.again) void loadThreadView(conversationId);
  }
}

/**
 * News waiting for the next frame: a streaming turn says a delta at a time, each its own task in
 * the WebView, and several sessions stream at once. A frame draws what came since the last one
 * once, not once per delta. A hidden window draws no frames: there it waits a moment instead.
 */
let queued: ThreadNews[] = [];
let frame = 0;
let timer: ReturnType<typeof setTimeout> | null = null;
const NEWS_WAIT_MS = 100;

function scheduleNews(): void {
  // Outside a page (the unit tests) there are no frames: the news goes on at once.
  const page = typeof document !== "undefined" && typeof requestAnimationFrame === "function";
  if (page && !document.hidden) frame = requestAnimationFrame(flushNews);
  // A frame that doesn't come (a window hidden meanwhile) doesn't hold the news back.
  timer = setTimeout(flushNews, page ? NEWS_WAIT_MS : 0);
}

function flushNews(): void {
  if (frame) cancelAnimationFrame(frame);
  if (timer !== null) clearTimeout(timer);
  frame = 0;
  timer = null;
  const batch = queued;
  queued = [];
  const { views } = useCliThread.getState();
  const next: Record<string, ThreadView> = {};
  const turned = new Set<string>();
  for (const news of batch) {
    const id = news.conversationId;
    if (news.opened || news.ended) turned.add(id);
    const read = reads.get(id);
    if (read) {
      read.heard.push(...news.entries);
      Object.assign(read.lists, listsOf(news));
      continue;
    }
    // A thread not read yet is read whole when it opens.
    const view = next[id] ?? views[id];
    const lists = listsOf(news);
    if (!view || (news.entries.length === 0 && Object.keys(lists).length === 0)) continue;
    next[id] = {
      ...view,
      ...(news.entries.length > 0 && { entries: mergeEntries(view.entries, news.entries) }),
      ...lists,
    };
  }
  if (Object.keys(next).length > 0) useCliThread.setState((state) => ({ views: { ...state.views, ...next } }));
  for (const id of turned) if (reads.has(id) || useCliThread.getState().views[id]) void loadThreadView(id);
}

/** What a CLI's live stream added to a conversation's thread (`threadNews`). */
export function applyThreadNews(news: ThreadNews): void {
  queued.push(news);
  if (queued.length === 1) scheduleNews();
}

/**
 * After (re)connecting: the news missed is read again with the shown conversations' threads;
 * the others are dropped and read when they open.
 */
export function resyncThreadViews(shown: readonly string[]): Promise<void> {
  const { views } = useCliThread.getState();
  useCliThread.setState({ views: Object.fromEntries(shown.flatMap((id) => (views[id] ? [[id, views[id]] as const] : []))) });
  return Promise.all(shown.map((id) => loadThreadView(id))).then(() => {});
}

/** A conversation's thread, read when first asked for; `null` until read. */
export function useThreadView(conversationId: string | null): ThreadView | null {
  const view = useCliThread((s) => (conversationId ? (s.views[conversationId] ?? null) : null));
  useEffect(() => {
    if (conversationId && !useCliThread.getState().views[conversationId]) void loadThreadView(conversationId);
  }, [conversationId]);
  return view;
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

/** A session's own workers, as their CLIs and git show them (S14), newest first. */
export function useWorkerRows(conversationId: string | null): readonly WorkerRow[] {
  return useCliThread((s) => (conversationId ? (s.views[conversationId] ? workersOf(conversationId, s.views[conversationId]) : NO_ROWS) : NO_ROWS));
}

/** The MCP servers a session's own CLI has, as it last said. */
export function useSessionMcp(conversationId: string): readonly McpServerState[] {
  return useCliThread((s) => s.views[conversationId]?.mcp ?? NO_SERVERS);
}

/** A worker's row, in whichever session's thread lists it. */
export function workerRowIn(views: Readonly<Record<string, ThreadView>>, taskId: string): { conversationId: string; row: WorkerRow } | null {
  for (const [conversationId, view] of Object.entries(views)) {
    const row = workersOf(conversationId, view).find((each) => each.id === taskId);
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

/** Reads worker `taskId`'s thread into `useWorkerThreads`. */
export async function loadWorkerThread(conversationId: string, taskId: string): Promise<void> {
  const { view } = await request({ method: "workerThread", conversationId, taskId });
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
