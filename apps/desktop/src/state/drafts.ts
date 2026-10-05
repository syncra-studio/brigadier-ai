import { request } from "@/ipc/client";
import type { AttachmentRef, Mention } from "@/ipc/generated";

/**
 * What the composer holds but hasn't sent, per conversation (and one for a new chat):
 * text and attachments come back when the conversation opens again,
 * after a restart too. The text lives in the webview's storage; the attachments' blobs are
 * pinned in the daemon so collection keeps them until the draft is sent or discarded.
 */
export type Draft = {
  text: string;
  attachments: AttachmentRef[];
  /** The files and conversations it @-mentions, each where its `@name` starts in the text. */
  mentions?: DraftMention[];
  atMs: number;
};

export type DraftMention = { at: number; name: string; mention: Mention };

/** The draft scope of a new chat (a conversation's is its id). */
export const NEW_CHAT_SCOPE = "new";

const STORAGE_KEY = "brigadier.drafts";
/** Drafts kept; the oldest go first (and let their attachments go). */
const MAX_DRAFTS = 50;

function readAll(): Record<string, Draft> {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    return stored ? (JSON.parse(stored) as Record<string, Draft>) : {};
  } catch {
    return {};
  }
}

function writeAll(drafts: Record<string, Draft>): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(drafts));
  } catch {
    // Storage can be full or unavailable; the draft just isn't kept.
  }
}

function pin(scope: string, attachments: AttachmentRef[]): void {
  request({ method: "pinDraftAttachments", scope, attachments }).catch((error: unknown) =>
    console.warn("could not keep the draft's attachments", error),
  );
}

const sameAttachments = (a: readonly AttachmentRef[], b: readonly AttachmentRef[]) =>
  a.length === b.length && a.every((ref, index) => ref.id === b[index]?.id);

export function loadDraft(scope: string): Draft | null {
  return readAll()[scope] ?? null;
}

/** Keeps what the composer holds now; an empty composer discards the draft. */
export function saveDraft(
  scope: string,
  text: string,
  attachments: AttachmentRef[],
  mentions: DraftMention[],
): void {
  // Retained undo/history refs must not keep a sent or cleared draft (and its pins) alive.
  if (text.trim() === "") attachments = attachments.filter((ref) => ref.inline == null);
  const drafts = readAll();
  const before = drafts[scope];
  if (text.trim() === "" && attachments.length === 0) {
    if (!before) return;
    delete drafts[scope];
  } else {
    drafts[scope] = { text, attachments, mentions, atMs: Date.now() };
  }
  const oldest = Object.entries(drafts)
    .toSorted(([, a], [, b]) => b.atMs - a.atMs)
    .slice(MAX_DRAFTS);
  for (const [evicted, draft] of oldest) {
    delete drafts[evicted];
    if (draft.attachments.length > 0) pin(evicted, []);
  }
  writeAll(drafts);
  if (!sameAttachments(before?.attachments ?? [], attachments)) pin(scope, attachments);
}

/** Drops a deleted conversation's draft (the daemon drops its pin with the conversation). */
export function forgetDraft(scope: string): void {
  const drafts = readAll();
  if (!(scope in drafts)) return;
  delete drafts[scope];
  writeAll(drafts);
}
