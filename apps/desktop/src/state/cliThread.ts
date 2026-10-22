import { useEffect, useMemo } from "react";
import { create } from "zustand";

import { foldTranscript, type TranscriptItem } from "@/components/conversation/transcript/transcript";
import { request } from "@/ipc/client";
import type { AttachmentRef, BackgroundTask, FileEntry, FileImage, ProviderEvent, ThreadNews, ThreadView } from "@/ipc/generated";

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

/**
 * `more` added to `entries`, one entry per item, as the engine merges them: a later event of an
 * item already there takes its place, which stays where the item began; a running command's
 * output grows its row. Entries left as they were keep their identity, so only the rows that
 * changed draw again.
 */
export function mergeEntries(entries: readonly FileEntry[], more: readonly FileEntry[]): FileEntry[] {
  const merged = [...entries];
  const at = new Map<string, number>();
  merged.forEach((entry, index) => {
    const id = itemIdOf(entry.event);
    if (id !== null) at.set(id, index);
  });
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

/**
 * Reads under way, with what the stream said meanwhile (items, and the CLI's background tasks
 * when they changed), and whether a turn ended meanwhile.
 */
const reads = new Map<string, { heard: FileEntry[]; background?: BackgroundTask[]; again: boolean }>();

function setView(conversationId: string, view: ThreadView): void {
  useCliThread.setState((state) => ({ views: { ...state.views, [conversationId]: view } }));
}

/**
 * Reads a conversation's thread. What the stream said while it was read is added once it lands:
 * whole items again (they only replace themselves), not deltas, which the view may hold already
 * (their item's own end brings its whole text). A turn that ended meanwhile reads it once more.
 */
export async function loadThreadView(conversationId: string): Promise<void> {
  const going = reads.get(conversationId);
  if (going) {
    going.again = true;
    return;
  }
  const read: { heard: FileEntry[]; background?: BackgroundTask[]; again: boolean } = { heard: [], again: false };
  reads.set(conversationId, read);
  try {
    const { view } = await request({ method: "threadView", conversationId });
    const heard = read.heard.filter((entry) => !isDelta(entry.event));
    // One without an id of its own (an error) is the view's already when it holds the same.
    const seen = new Set(view.entries.filter((entry) => itemIdOf(entry.event) === null).map((entry) => JSON.stringify(entry)));
    const fresh = heard.filter((entry) => itemIdOf(entry.event) !== null || !seen.has(JSON.stringify(entry)));
    const landed = fresh.length > 0 ? { ...view, entries: mergeEntries(view.entries, fresh) } : view;
    setView(conversationId, read.background ? { ...landed, background: read.background } : landed);
  } catch (error) {
    console.error("reading the thread failed", error);
  } finally {
    reads.delete(conversationId);
    if (read.again) void loadThreadView(conversationId);
  }
}

/** News waiting for the next microtask: a streaming turn says a delta at a time. */
let queued: ThreadNews[] = [];

function flushNews(): void {
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
      if (news.background) read.background = news.background;
      continue;
    }
    // A thread not read yet is read whole when it opens.
    const view = next[id] ?? views[id];
    if (!view || (news.entries.length === 0 && !news.background)) continue;
    next[id] = {
      ...view,
      ...(news.entries.length > 0 && { entries: mergeEntries(view.entries, news.entries) }),
      ...(news.background && { background: news.background }),
    };
  }
  if (Object.keys(next).length > 0) useCliThread.setState((state) => ({ views: { ...state.views, ...next } }));
  for (const id of turned) if (reads.has(id) || useCliThread.getState().views[id]) void loadThreadView(id);
}

/** What a CLI's live stream added to a conversation's thread (`threadNews`). */
export function applyThreadNews(news: ThreadNews): void {
  queued.push(news);
  if (queued.length === 1) queueMicrotask(flushNews);
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
