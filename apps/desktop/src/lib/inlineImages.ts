import type { Unstable_DirectiveSegment } from "@assistant-ui/react";
import type { AttachmentRef } from "@/ipc/generated";

export const INLINE_IMAGE = "image";
export const IMAGE_UPLOAD = "image-upload";

export function isInlineImage(mime: string): boolean {
  return ["image/png", "image/jpeg", "image/gif", "image/webp"].includes(mime);
}

export const imageToken = (id: string): string => `[image:${id}]`;
export const uploadToken = (key: string): string => `[image-upload:${key}]`;

export function imageTokens(text: string): { id: string; start: number; end: number }[] {
  return [...text.matchAll(/\[image:([^[\]]*)\]/g)].map((match) => ({
    id: match[1]!, start: match.index, end: match.index + match[0].length,
  }));
}

/** Only known, eligible inline refs count; repeated tokens share a single ref. */
export function inlineRefs(text: string, refs: Iterable<AttachmentRef>): AttachmentRef[] {
  const known = new Map([...refs].filter((ref) => ref.inline && isInlineImage(ref.mime)).map((ref) => [ref.id, ref]));
  return [...new Set(imageTokens(text).map(({ id }) => id))].flatMap((id) => {
    const ref = known.get(id);
    return ref ? [{ ...ref, inline: true, pasted: false }] : [];
  });
}

/** The daemon's limit per attachment list, draft pins included (MAX_ATTACHMENTS in the core). */
export const MAX_ATTACHMENTS = 20;

/**
 * What a draft pins: its row files and the images in its text, then retained images (newest
 * first, so undo can bring them back) while the daemon's limit allows.
 */
export function draftRefs(
  text: string,
  rows: readonly AttachmentRef[],
  retained: Iterable<AttachmentRef>,
): AttachmentRef[] {
  const kept = [...retained];
  const shown = inlineRefs(text, kept);
  const ids = new Set(shown.map((ref) => ref.id));
  const older = kept.filter((ref) => !ids.has(ref.id)).toReversed();
  return [...rows, ...shown, ...older].slice(0, MAX_ATTACHMENTS);
}

/** Sent edits retain every row ref, even when it shares an inline image's id. */
export function reconcileImages(text: string, refs: readonly AttachmentRef[]): AttachmentRef[];
export function reconcileImages(text: string, refs: readonly AttachmentRef[] | undefined): AttachmentRef[] | undefined;
export function reconcileImages(text: string, refs: readonly AttachmentRef[] | undefined): AttachmentRef[] | undefined {
  // An unavailable original must let the backend reuse its attachments.
  if (refs === undefined) return undefined;
  return [...refs.filter((ref) => !ref.inline), ...inlineRefs(text, refs)];
}

/** Queue edits own their row attachments; only inline refs can come from the original. */
export function queuedImageRefs(
  text: string,
  original: readonly AttachmentRef[],
  edited: readonly AttachmentRef[],
): AttachmentRef[] {
  return reconcileImages(text, [...original.filter((ref) => ref.inline), ...edited]);
}

export type ImageUpload =
  | { status: "pending"; name: string }
  | { status: "failed"; name: string; error: string }
  | { status: "ready"; name: string; ref: AttachmentRef };

/** Scoped to a composer. Retained refs outlive nodes, for undo, cut/paste and draft pins. */
export class InlineImages {
  readonly refs = new Map<string, AttachmentRef>();
  readonly uploads = new Map<string, ImageUpload>();
  private active = new Set<string>();
  private listeners = new Set<() => void>();
  private version = 0;
  snapshot = (): number => this.version;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private changed(): void {
    this.version += 1;
    for (const listener of this.listeners) listener();
  }
  get blocked(): boolean {
    return [...this.active].some((key) => this.uploads.get(key)?.status !== "ready");
  }
  retain(refs: Iterable<AttachmentRef>): void {
    let changed = false;
    for (const ref of refs) {
      if (!ref.inline || !isInlineImage(ref.mime) || this.refs.has(ref.id)) continue;
      this.refs.set(ref.id, ref);
      changed = true;
    }
    if (changed) this.changed();
  }
  /** Restored uploads cannot resume after a reload; keep a removable error at their place. */
  recall(text: string, refs: Iterable<AttachmentRef>): void {
    this.retain(refs);
    for (const match of text.matchAll(/\[image-upload:([^[\]]*)\]/g)) {
      const key = match[1]!;
      if (this.uploads.has(key)) continue;
      this.uploads.set(key, { status: "failed", name: "Image", error: "Image upload interrupted. Remove it and paste again." });
      this.active.add(key);
    }
    this.changed();
  }
  begin(key: string, name: string): void {
    this.uploads.set(key, { status: "pending", name });
    this.active.add(key);
    this.changed();
  }
  /** Deleting a pending node cancels its completion, including after an undo. */
  sync(keys: Iterable<string>): void {
    const next = new Set(keys);
    if (next.size === this.active.size && [...next].every((key) => this.active.has(key))) return;
    for (const key of this.active) {
      const upload = this.uploads.get(key);
      if (!next.has(key) && upload?.status === "pending") {
        this.uploads.set(key, { status: "failed", name: upload.name, error: "Image upload canceled. Remove it and paste again." });
      }
    }
    this.active = next;
    this.changed();
  }
  complete(key: string, ref: AttachmentRef): boolean {
    const upload = this.uploads.get(key);
    if (!this.active.has(key) || upload?.status !== "pending") return false;
    const inline = { ...ref, inline: true, pasted: false };
    this.refs.set(inline.id, inline);
    this.uploads.set(key, { status: "ready", name: upload.name, ref: inline });
    this.changed();
    return true;
  }
  fail(key: string, error: unknown): void {
    const upload = this.uploads.get(key);
    if (!this.active.has(key) || upload?.status !== "pending") return;
    this.uploads.set(key, { status: "failed", name: upload.name, error: error instanceof Error ? error.message : String(error) });
    this.changed();
  }
  close(): void { this.sync([]); }
}

/** Image segments take precedence over other chips, as the backend counts tokens everywhere. */
export function imageSegments(text: string, images: InlineImages | undefined, plain: (text: string) => Unstable_DirectiveSegment[]): Unstable_DirectiveSegment[] {
  const segments: Unstable_DirectiveSegment[] = [];
  let at = 0;
  for (const match of text.matchAll(/\[(image|image-upload):([^[\]]*)\]/g)) {
    const type = match[1]!;
    const id = match[2]!;
    const ref = images?.refs.get(id);
    const upload = images?.uploads.get(id);
    if (type === INLINE_IMAGE ? !ref : !upload) continue;
    segments.push(...plain(text.slice(at, match.index)));
    segments.push({ kind: "mention", type, id, label: ref?.name ?? upload?.name ?? id });
    at = match.index + match[0].length;
  }
  segments.push(...plain(text.slice(at)));
  return segments;
}
