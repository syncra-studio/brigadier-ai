// The Electron shell's browser_agent commands (src/state/agentBrowser.ts's hands): the script looks, the cursor glides
// to what it found, and real input goes in under the focus guard. The main process holds the debugger.
import { glideTo, pressAt, raiseShield, dropOverlay } from "./agentOverlay";
import { guardFocus, userComposing } from "./agentFocusGuard";
import { agentPage, focusPage, loadPage, pageBounds, whilePainting } from "./tabLayer";
import { isPaused, userUsed } from "@/state/agentPause";

type Answer = Record<string, unknown>;
const agent = (call: Record<string, unknown>) => window.brigadierShell.agent(call);

const script = (guest: number, op: Answer) => agent({ call: "script", guest, op }) as Promise<Answer | null>;

/** Reads a page, or acts on one of its controls. */
export async function agentRun(id: string, op: Answer): Promise<unknown> {
  const { guest, element } = agentPage(id);
  if (op.kind !== "click" && op.kind !== "type") return script(guest, op);
  const answer = await script(guest, { ...op, trusted: true });
  const point = (answer as { point?: { x: number; y: number } } | null)?.point;
  if (!point) return answer;

  const zoom = element.getZoomFactor();
  await glideTo(id, String(op.label ?? "Brigadier"), { x: point.x * zoom, y: point.y * zoom });
  if (isPaused(id)) return { paused: "user" };

  // The page may have moved while the cursor went; a field must still be the one with the focus.
  const place = await script(guest, { kind: "place", target: op.target, typing: op.kind === "type" });
  if (place && "error" in place) return place;
  const there = (place as { point?: { x: number; y: number } } | null)?.point ?? point;

  if (isPaused(id)) return { paused: "user" };
  if (userComposing()) return { error: "The user is typing with an input method in Brigadier, so nothing was sent, to keep their unfinished text. Try again in a moment." };
  const release = guardFocus(element, () => focusPage(null));
  try {
    if (op.kind === "type") {
      const typed = await script(guest, { kind: "insert", target: op.target, text: op.text });
      return typed && "error" in typed || op.submit !== true ? typed : await agent({ call: "enter", guest });
    }
    if (Math.abs(there.x - point.x) > 0.5 || Math.abs(there.y - point.y) > 0.5) await glideTo(id, String(op.label ?? "Brigadier"), { x: there.x * zoom, y: there.y * zoom });
    pressAt(id);
    return await agent({ call: "click", guest, x: there.x * zoom, y: there.y * zoom });
  } finally {
    release();
  }
}

/** An agent has the tab now (the shield goes up), or doesn't (it comes down, and the debugger goes). */
export async function agentDrive(id: string, on: boolean, resumed: boolean): Promise<void> {
  if (!on) {
    dropOverlay(id);
    const page = maybePage(id);
    if (page) await agent({ call: "release", guest: page.guest });
    return;
  }
  const page = maybePage(id);
  if (!page) return;
  raiseShield(id, page.element, pageBounds(id), () => {
    // The user's own first click or key: the tab is theirs, and so is the keyboard.
    dropOverlay(id);
    userUsed(id);
    page.element.focus();
  }, ({ x, y, deltaX, deltaY }) => void agent({ call: "wheel", guest: page.guest, x, y, deltaX, deltaY }));
  if (!resumed) return;
  // Continue: the hold comes back as it was, and refs from before the user's turn mean nothing now.
  await agent({ call: "attach", guest: page.guest });
  await script(page.guest, { kind: "void" });
}

function maybePage(id: string) {
  try {
    return agentPage(id);
  } catch {
    return null;
  }
}

export function agentLoad(id: string, url: string): void {
  if (!/^https?:/i.test(url)) throw new Error(`${url} isn't a web address.`);
  loadPage(id, url);
}

export function agentEvents(id: string, since: number, kinds: readonly string[]): Promise<unknown> {
  return agent({ call: "events", guest: agentPage(id).guest, since, kinds });
}

export async function agentCapture(id: string): Promise<ArrayBuffer> {
  const { guest } = agentPage(id);
  const png = await whilePainting(id, () => agent({ call: "capture", guest })) as Uint8Array;
  return png.buffer.slice(png.byteOffset, png.byteOffset + png.byteLength) as ArrayBuffer;
}
