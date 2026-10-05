/**
 * Images pasted into the composer's text. A pasted image (a screenshot, an image copied from a
 * page or an app) goes where the caret is, as an `[Image #n]` chip, and goes to the model
 * there; files pasted, picked or dropped go in the attachments row above the text instead.
 */

/** What stands for pasted image `n` in the message's text. */
export function imageMarker(n: number): string {
  return `[Image #${n}]`;
}

const MARKER = /\[Image #(\d+)\]/g;

/** The pasted images' numbers whose `[Image #n]` is in `text`. */
export function imageMarkersIn(text: string): Set<number> {
  return new Set([...text.matchAll(MARKER)].map((match) => Number(match[1])));
}

/** The `[Image #n]` starting at `at` in `text`, if one does: its number and length. */
export function imageMarkerAt(text: string, at: number): { n: number; length: number } | null {
  if (text[at] !== "[") return null;
  const match = /^\[Image #(\d+)\]/.exec(text.slice(at, at + 24));
  return match?.[1] ? { n: Number(match[1]), length: match[0].length } : null;
}

/** `text` in pieces around its `[Image #n]` markers: the text between them, and each `n`. */
export function splitAtImages(text: string): (string | number)[] {
  return text
    .split(/(\[Image #\d+\])/)
    .map((piece, index) => (index % 2 === 1 ? Number(piece.slice(8, -1)) : piece))
    .filter((piece) => piece !== "");
}

/** `text` as words: each `[Image #n]` of an image pasted into it (`numbers`) as "Image #n". */
export function namedImages(text: string, numbers: Iterable<number>): string {
  let words = text;
  for (const n of numbers) words = words.replaceAll(imageMarker(n), `Image #${n}`);
  return words;
}

/** The number for the next pasted image: one past the highest in use. */
export function nextImageNumber(taken: Iterable<number>): number {
  let highest = 0;
  for (const n of taken) highest = Math.max(highest, n);
  return highest + 1;
}

/**
 * The numbers the images copied with pasted text take in the composer (`copied`, in the order
 * pasted): each keeps its own unless that is `taken`, else gets the next free one.
 */
export function pastedImageNumbers(
  copied: Iterable<number>,
  taken: Iterable<number>,
): Map<number, number> {
  const used = new Set(taken);
  const numbers = new Map<number, number>();
  for (const n of copied) {
    const next = used.has(n) ? nextImageNumber(used) : n;
    used.add(next);
    numbers.set(n, next);
  }
  return numbers;
}

/**
 * The just-pasted images that are an image the composer has already, the same bytes, so each
 * image has one number however often it appears. `pasted` and `others` are numbers with their
 * content hashes (`others`: the composer's other images, in the text or deleted but such as undo
 * could bring back). Each pasted number whose hash is another image's, or an earlier pasted
 * one's, maps to that image's number.
 */
export function sameImages(
  pasted: Iterable<readonly [number, string]>,
  others: Iterable<readonly [number, string]>,
): Map<number, number> {
  const numbers = new Map<string, number>();
  for (const [n, hash] of others) numbers.set(hash, Math.min(n, numbers.get(hash) ?? n));
  const same = new Map<number, number>();
  for (const [k, hash] of pasted) {
    const m = numbers.get(hash);
    if (m === undefined) numbers.set(hash, k);
    else if (m !== k) same.set(k, m);
  }
  return same;
}

/**
 * Pasted `text` with each image that came with it renumbered (`numbers`). The marker of an
 * image that didn't come with it (text copied from elsewhere) becomes plain words, so no
 * marker goes out without its image.
 */
export function renumberImages(text: string, numbers: ReadonlyMap<number, number>): string {
  return text.replace(MARKER, (_, digits: string) => {
    const next = numbers.get(Number(digits));
    return next === undefined ? `Image #${digits}` : imageMarker(next);
  });
}

/**
 * How long ago a file the clipboard made up for image data was made (its `lastModified`): the
 * webview makes it at the paste, so it is a moment old. A copied file was saved before.
 */
const FRESH_MS = 2_000;

/** What the composer reads off a paste. */
export type PastedData = {
  files: readonly Pick<File, "name" | "type" | "lastModified">[];
  /** The paste's `text/uri-list`. */
  uris: string;
};

/**
 * Whether a paste is images themselves, to go inline at the caret: every file is an image
 * the clipboard holds as data (a screenshot, "Copy Image"), not a file copied in Finder or
 * Explorer. Image data comes as a file the webview makes up at the paste ("image.png", dated
 * now); a copied file keeps its own date, even one named image.png, and may come with its
 * `file:` URL.
 */
export function pastesInline({ files, uris }: PastedData, now = Date.now()): boolean {
  if (files.length === 0 || /^file:/im.test(uris)) return false;
  return files.every(
    (file) => file.type.startsWith("image/") && Math.abs(now - file.lastModified) < FRESH_MS,
  );
}
