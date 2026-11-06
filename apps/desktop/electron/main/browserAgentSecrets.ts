// What keeps a password field's value from an agent, past the script's own hiding: known values scrubbed from what
// its verbs answer, and fields painted out of its pictures. The values stay here in the main process, in memory, for
// as long as the tab lives, and no call returns them. Best effort by nature: a value the page shows as text in another
// form, or a field nothing can recognise, passes.
import { nativeImage, type NativeImage, type WebContents, type WebFrameMain } from "electron";

import agentScript from "../../src-tauri/src/browser_agent.js?raw";

/** The script's own world: the page can't reach its variables, or stand in for what it calls. */
const AGENT_WORLD = 1010;
const scriptBody = agentScript.trim().replace(/;$/, "");
/** The agent's script, for what runs it elsewhere: the eval verb's answer and the picker's description. */
export const agentScriptBody = scriptBody;
const call = (op: object) => `(${scriptBody})(${JSON.stringify(op)})`;

/** Runs the agent's script on the tab's page, in its own world. The op is data, and the script is the bundle's own. */
export function runScript(guest: WebContents, op: object): Promise<unknown> {
  return guest.executeJavaScriptInIsolatedWorld(AGENT_WORLD, [{ code: call({ ...op, isolated: true }) }]);
}

type Rect = { x: number; y: number; width: number; height: number };
type Sweep = { fields: Rect[]; values: string[]; width: number; frames: { origin: string; rect: Rect }[] };

/** A value this short would blank ordinary text wherever it occurs. */
const SHORTEST = 3;
const KEPT = 50;
const HIDDEN = "(hidden)";
const FRAME_WAIT_MS = 1500;
const SWEEP_AFTER_MS = 400;

const known = new Map<number, Set<string>>();
const sweepsDue = new Map<number, ReturnType<typeof setTimeout>>();

function keep(guest: number, values: readonly string[]): void {
  let kept = known.get(guest);
  for (const value of values) {
    if (value.length < SHORTEST) continue;
    if (!kept) known.set(guest, (kept = new Set()));
    kept.delete(value);
    kept.add(value);
    if (kept.size > KEPT) kept.delete(kept.values().next().value as string);
  }
}

/** A subframe's sweep runs in its own main world, the one way into it from here: it only reads. Answers null if it can't. */
function sweepFrame(frame: WebFrameMain): Promise<Sweep | null> {
  const asked = frame.executeJavaScript(call({ kind: "sweep" })).then((answer) => answer as Sweep, () => null);
  return Promise.race([asked, new Promise<null>((resolve) => setTimeout(() => resolve(null), FRAME_WAIT_MS))]);
}

const originOf = (url: string, fallback: string) => {
  try {
    const { origin } = new URL(url);
    return origin === "null" ? fallback : origin;
  } catch {
    return fallback;
  }
};

/** The page's password fields and every frame's, with what they hold kept. Rejects if the page itself can't be looked at. */
async function sweep(guest: WebContents): Promise<{ rects: Rect[]; width: number }> {
  const main = await runScript(guest, { kind: "sweep" }) as Sweep;
  keep(guest.id, main.values);
  const rects = [...main.fields];
  const pageOrigin = originOf(guest.getURL(), "");
  const top = guest.mainFrame;
  const subframes = top.framesInSubtree.filter((frame) => frame !== top);
  const answers = await Promise.all(subframes.map(sweepFrame));
  const suspects = new Set<string>();
  answers.forEach((answer, index) => {
    if (answer) keep(guest.id, answer.values);
    // A frame's own coordinates mean nothing here, so a frame holding a field, or one that can't say, goes whole.
    if (answer && answer.fields.length === 0) return;
    let outer = subframes[index]!;
    while (outer.parent && outer.parent !== top) outer = outer.parent;
    suspects.add(originOf(outer.url, pageOrigin));
  });
  if (suspects.size > 0) {
    const matching = main.frames.filter((frame) => suspects.has(frame.origin));
    rects.push(...(matching.length > 0 ? matching : main.frames).map((frame) => frame.rect));
  }
  return { rects, width: main.width };
}

/** Looks for values soon: a page that logs often is looked at once for a burst. */
export function sweepSoon(guest: WebContents): void {
  if (sweepsDue.has(guest.id)) return;
  sweepsDue.set(guest.id, setTimeout(() => {
    sweepsDue.delete(guest.id);
    if (!guest.isDestroyed()) sweep(guest).catch(() => {});
  }, SWEEP_AFTER_MS));
}

/** Looks now: the document that holds them may be about to go. */
export function sweepNow(guest: WebContents): void {
  sweep(guest).catch(() => {});
}

export function forgetSecrets(guest: number): void {
  known.delete(guest);
  const due = sweepsDue.get(guest);
  if (due) clearTimeout(due);
  sweepsDue.delete(guest);
}

function scrubText(text: string, values: readonly string[]): string {
  let scrubbed = text;
  for (const value of values) scrubbed = scrubbed.split(value).join(HIDDEN);
  return scrubbed;
}

/** `answer` with every value known on the tab taken out of its strings, as written and as an address or form writes it. */
export function scrubbed<T>(guest: number, answer: T): T {
  const kept = known.get(guest);
  if (!kept) return answer;
  const forms = new Set<string>();
  for (const value of kept) {
    forms.add(value);
    forms.add(encodeURIComponent(value));
    forms.add(encodeURIComponent(value).replace(/%20/g, "+"));
  }
  const values = [...forms].sort((a, b) => b.length - a.length);
  const walk = (value: unknown): unknown => {
    if (typeof value === "string") return scrubText(value, values);
    if (Array.isArray(value)) return value.map(walk);
    if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, walk(item)]));
    return value;
  };
  return walk(answer) as T;
}

/** Looks for values, then answers with them scrubbed. A failed look still scrubs what is known. */
export async function sweptAndScrubbed<T>(guest: WebContents, answer: () => T | Promise<T>): Promise<T> {
  await sweep(guest).catch(() => {});
  return scrubbed(guest.id, await answer());
}

function paint(image: NativeImage, rects: readonly Rect[], pageWidth: number): Buffer {
  const scaleFactor = image.getScaleFactors()[0] ?? 1;
  const { width, height } = image.getSize(scaleFactor);
  const bitmap = image.toBitmap({ scaleFactor });
  const scale = width / pageWidth;
  for (const rect of rects) {
    const left = Math.max(0, Math.floor(rect.x * scale) - 1);
    const right = Math.min(width, Math.ceil((rect.x + rect.width) * scale) + 1);
    const top = Math.max(0, Math.floor(rect.y * scale) - 1);
    const bottom = Math.min(height, Math.ceil((rect.y + rect.height) * scale) + 1);
    for (let y = top; y < bottom; y++) {
      for (let x = left; x < right; x++) {
        const at = (y * width + x) * 4;
        // Black in either byte order, opaque.
        bitmap[at] = 0;
        bitmap[at + 1] = 0;
        bitmap[at + 2] = 0;
        bitmap[at + 3] = 255;
      }
    }
  }
  return nativeImage.createFromBitmap(bitmap, { width, height, scaleFactor }).toPNG({ scaleFactor });
}

/**
 * The page's picture for an agent, its password fields painted out, by looks before and after it, so a field that moves
 * while it is taken stays covered. A page that can't be looked at gives no picture.
 */
export async function pictureForAgent(guest: WebContents): Promise<Buffer> {
  const before = await sweep(guest).catch(() => null);
  const image = await guest.capturePage();
  const after = await sweep(guest).catch(() => null);
  if (!before || !after) throw new Error("The page's password fields couldn't be found, so no picture was taken. Try again in a moment.");
  return paint(image, [...before.rects, ...after.rects], after.width);
}
