// What an agent's verbs do to a guest (crates/core's browser tool, src/state/agentBrowser.ts). Agents only ever have
// verbs: nothing here takes protocol methods, so the app's page can ask for a click, never for a CDP call. Only the
// eval verb takes code, which main runs as the page's own script (browserDevtoolsEval.ts). Pages, network and console come from Electron's own events, with no debugger on the tab;
// webContents.debugger is attached only while an agent acts, for trusted input, and goes as soon as the user takes
// the tab. It is in-process, so no port and no automation switch.
import { webContents, type WebContents } from "electron";

import { handleFromApp } from "./appPage";
import { activityOf, forgetActivity, watchActivity, watchNetwork } from "./browserAgentActivity";
import { forgetSecrets, pictureForAgent, runScript, sweptAndScrubbed } from "./browserAgentSecrets";
import { evalOnPage } from "./browserDevtoolsEval";

export { watchNetwork };

type Held = {
  /** Calls run in order: a let-go waits for a click under way, so no button stays pressed. */
  queue: Promise<unknown>;
};
const held = new Map<number, Held>();

/** The guest of a call from `sender`, which must be the page hosting it. */
/** Tabs with DevTools open. `isDevToolsOpened` answers false for DevTools in a view of ours (browserDevtools.ts). */
const opened = new Set<number>();
export const devToolsOpen = (guest: WebContents): boolean => opened.has(guest.id) || guest.isDevToolsOpened();

function guestFor(sender: WebContents, id: unknown): WebContents {
  const guest = typeof id === "number" ? webContents.fromId(id) : undefined;
  if (!guest || guest.isDestroyed() || guest.getType() !== "webview" || guest.hostWebContents !== sender) throw new Error("That page is gone.");
  return guest;
}

function tellHost(guest: WebContents, event: "devtools-opened" | "devtools-closed" | "detached"): void {
  guest.hostWebContents?.send("brigadier:agent-event", { from: guest.id, event });
}

async function attach(guest: WebContents): Promise<void> {
  if (held.has(guest.id)) return;
  if (devToolsOpen(guest)) throw new Error("devtools_open");
  guest.debugger.attach("1.3");
  held.set(guest.id, { queue: Promise.resolve() });
  // A page behind its tab takes input and keeps its timers, and the user's focus going back blurs nothing. The same
  // use as Orca's enablePageFocusEmulation (MIT, stablyai/orca). What this changes goes with the session.
  await guest.debugger.sendCommand("Emulation.setFocusEmulationEnabled", { enabled: true }).catch(() => {});
}

/** Lets go of the tab, after what is under way. Ending the session clears any input state and the emulation. */
async function release(guest: WebContents): Promise<void> {
  const hold = held.get(guest.id);
  if (!hold) return;
  held.delete(guest.id);
  await hold.queue.catch(() => {});
  if (!guest.isDestroyed() && guest.debugger.isAttached()) guest.debugger.detach();
}

/** Runs `input` in the tab's order; an answer the user's take-over overtook is `paused`. */
async function acting(guest: WebContents, input: (send: (method: string, params: object) => Promise<unknown>) => Promise<void>): Promise<unknown> {
  if (devToolsOpen(guest)) return { paused: "devtools" };
  await attach(guest);
  const hold = held.get(guest.id);
  if (!hold) return { paused: "user" };
  const run = hold.queue.then(async () => {
    const stillHeld = () => held.get(guest.id) === hold;
    if (!stillHeld()) return { paused: devToolsOpen(guest) ? "devtools" : "user" };
    await input((method, params) => guest.debugger.sendCommand(method, params));
    return { done: true };
  });
  hold.queue = run.catch(() => {});
  try {
    return await run;
  } catch (error) {
    if (guest.isDestroyed()) throw new Error("The tab closed.", { cause: error });
    if (devToolsOpen(guest) || !guest.debugger.isAttached()) return { paused: devToolsOpen(guest) ? "devtools" : "user" };
    throw error;
  }
}

const mouse = (type: string, x: number, y: number, buttons: number) => ({
  type,
  x,
  y,
  buttons,
  button: type === "mouseMoved" ? "none" : "left",
  clickCount: type === "mouseMoved" ? 0 : 1,
});

/** DevTools ends a debugger session (electron.d.ts, Debugger's `detach`), so opening it counts as take-over. */
function watchDevTools(guest: WebContents): void {
  guest.on("devtools-opened", () => {
    opened.add(guest.id);
    tellHost(guest, "devtools-opened");
    void release(guest);
  });
  guest.on("devtools-closed", () => devToolsEnded(guest));
  guest.debugger.on("detach", () => {
    // Ours forgets the hold first, so a hold still here was ended from outside: nothing resumes by itself.
    if (!held.delete(guest.id) || guest.isDestroyed()) return;
    tellHost(guest, devToolsOpen(guest) ? "devtools-opened" : "detached");
  });
}

/**
 * For whatever opens DevTools (section 9): pause first and release held input, then open. The in-flight call answers
 * "Paused: DevTools open".
 */
export async function beforeDevTools(guest: WebContents): Promise<void> {
  if (!held.has(guest.id)) return;
  tellHost(guest, "devtools-opened");
  await release(guest);
}

/** DevTools went from this tab. Closing DevTools held in a view of ours fires no `devtools-closed`, so its view's end says so. */
export function devToolsEnded(guest: WebContents): void {
  opened.delete(guest.id);
  if (!guest.isDestroyed()) tellHost(guest, "devtools-closed");
}

/** Every tab's guest, from when it is made. */
export function watchGuest(guest: WebContents): void {
  const id = guest.id;
  watchActivity(guest);
  watchDevTools(guest);
  guest.once("destroyed", () => {
    held.delete(id);
    opened.delete(id);
    forgetActivity(id);
    forgetSecrets(id);
  });
}

type Call =
  | { call: "script"; guest: number; op: Record<string, unknown> }
  | { call: "attach"; guest: number }
  | { call: "release"; guest: number }
  | { call: "click"; guest: number; x: number; y: number }
  | { call: "enter"; guest: number }
  | { call: "wheel"; guest: number; x: number; y: number; deltaX: number; deltaY: number }
  | { call: "events"; guest: number; since: number; kinds: string[] }
  | { call: "capture"; guest: number };

export function installBrowserAgent(): void {
  handleFromApp("brigadier:agent", async (event, call: Call) => {
    const guest = guestFor(event.sender, call.guest);
    switch (call.call) {
      case "script":
        // A sweep's answer holds field values, and stays in this process.
        if (call.op.kind === "sweep") throw new Error("Unknown agent call");
        // The one way code from outside reaches a page: the user read it on a card and said yes (agentBrowser.ts).
        if (call.op.kind === "eval") return evalOnPage(guest, String(call.op.script));
        return sweptAndScrubbed(guest, () => runScript(guest, call.op));
      case "attach":
        return attach(guest);
      case "release":
        return release(guest);
      case "click":
        return acting(guest, async (send) => {
          await send("Input.dispatchMouseEvent", mouse("mouseMoved", call.x, call.y, 0));
          await send("Input.dispatchMouseEvent", mouse("mousePressed", call.x, call.y, 1));
          await send("Input.dispatchMouseEvent", mouse("mouseReleased", call.x, call.y, 0));
        });
      case "enter":
        // Widget events, not the debugger's keys, which follow the window's focus (as Orca found, its #20595).
        return acting(guest, async () => {
          guest.sendInputEvent({ type: "keyDown", keyCode: "Return" });
          guest.sendInputEvent({ type: "char", keyCode: "Return" });
          guest.sendInputEvent({ type: "keyUp", keyCode: "Return" });
        });
      case "wheel":
        // The user's own scroll, which the shield over a driven tab passes on: no debugger, and it pauses nothing.
        guest.sendInputEvent({ type: "mouseWheel", x: Math.round(call.x), y: Math.round(call.y), deltaX: -call.deltaX, deltaY: -call.deltaY });
        return null;
      case "events":
        return sweptAndScrubbed(guest, () => activityOf(guest.id, call.since, call.kinds));
      case "capture":
        return pictureForAgent(guest);
      default:
        throw new Error("Unknown agent call");
    }
  });
}
