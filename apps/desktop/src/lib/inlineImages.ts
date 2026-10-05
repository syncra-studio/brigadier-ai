import type { AttachmentRef } from "@/ipc/generated";

export function isInlineImage(mime: string): boolean {
  return ["image/png", "image/jpeg", "image/gif", "image/webp"].includes(mime);
}

export const imageToken = (id: string): string => `[image:${id}]`;

export function imageTokens(text: string): { id: string; start: number; end: number }[] {
  return [...text.matchAll(/\[image:([^[\]]*)\]/g)].map((match) => ({
    id: match[1]!, start: match.index, end: match.index + match[0].length,
  }));
}

/** Only known, eligible inline refs count; repeated tokens share a single ref. */
export function inlineRefs(text: string, refs: Iterable<AttachmentRef>): AttachmentRef[] {
  const all = [...refs];
  const numbered = new Set([...text.matchAll(/\[Image #(\d+)\]/g)].map((match) => Number(match[1])));
  const selected = [...new Map(all.filter((ref) => ref.inline !== null && numbered.has(ref.inline)).map((ref) => [ref.inline, ref])).values()];
  const known = new Map(all.filter((ref) => ref.inline !== null && isInlineImage(ref.mime)).map((ref) => [ref.id, ref]));
  return [...selected, ...[...new Set(imageTokens(text).map(({ id }) => id))].flatMap((id) => {
    const ref = known.get(id);
    return ref ? [{ ...ref, inline: 0, pasted: false }] : [];
  })];
}

/** Plain text for previews and tooltips; only this message's eligible refs count. */
export function imagePreview(text: string, refs: Iterable<AttachmentRef>): string {
  const all = [...refs];
  const known = new Set(inlineRefs(text, all).map((ref) => ref.id));
  let shown = text;
  for (const ref of inlineRefs(text, all)) {
    if (ref.inline !== null && ref.inline > 0) shown = shown.replaceAll(`[Image #${ref.inline}]`, "[image]");
  }
  return shown.replace(/\[image:([^[\]]*)\]/g, (token, id: string) => known.has(id) ? "[image]" : token);
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
  return [...refs.filter((ref) => ref.inline === null), ...inlineRefs(text, refs)];
}

/** Queue edits own their row attachments; only inline refs can come from the original. */
export function queuedImageRefs(
  text: string,
  original: readonly AttachmentRef[],
  edited: readonly AttachmentRef[],
): AttachmentRef[] {
  return reconcileImages(text, [...original.filter((ref) => ref.inline !== null), ...edited]);
}

/** Upgrade main's stored ID tokens (including local drafts with boolean inline flags). */
export function normalizedImages(text: string, refs: readonly AttachmentRef[]): { text: string; attachments: AttachmentRef[] } {
  const taken = refs.flatMap((ref) => typeof ref.inline === "number" && ref.inline > 0 ? [ref.inline] : []);
  let next = Math.max(0, ...taken) + 1;
  const numbers = new Map<string, number>();
  const attachments = refs.map((ref) => {
    if (ref.inline === null || (ref.inline as unknown) === false || ref.inline === undefined) return { ...ref, inline: null };
    if (typeof ref.inline === "number" && ref.inline > 0) return ref;
    const n = numbers.get(ref.id) ?? next++;
    numbers.set(ref.id, n);
    return { ...ref, inline: n };
  });
  return { text: text.replace(/\[image:([^[\]]*)\]/g, (token, id: string) => {
    const n = numbers.get(id);
    return n === undefined ? token : `[Image #${n}]`;
  }), attachments };
}
