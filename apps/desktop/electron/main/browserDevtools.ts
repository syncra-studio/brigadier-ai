// Section 9: DevTools docked in a WebContentsView of the window, a device's size and user agent, and the element picker.
// DevTools opens through beforeDevTools (L128): on a driven tab that is a take-over, so the in-flight call answers
// "Paused: DevTools open". Device sizes need no debugger. The picker holds the debugger for as long as the user picks.
//
// Not a <webview>: Electron binds DevTools' host to the contents' current frame as its first load commits, so a contents
// that has navigated comes up unconnected (inspectable_web_contents.cc, ReadyToCommitNavigation), and a <webview> can't
// attach without a load. Electron skips its own test of that case. A view lies above the app's page, so the app hides it
// while a menu or dialog is over it (BrowserDevTools.tsx).
import { BrowserWindow, webContents, WebContentsView, type IpcMainInvokeEvent, type WebContents } from "electron";

import { handleFromApp } from "./appPage";
import { beforeDevTools, devToolsEnded, devToolsOpen } from "./browserAgent";
import { agentScriptBody, sweptAndScrubbed } from "./browserAgentSecrets";

/** The session of DevTools' front ends, in memory: it isn't a profile and holds nothing of one. */
const FRONT_PARTITION = "brigadier-devtools";

function guestFor(sender: WebContents, id: unknown): WebContents {
  const guest = typeof id === "number" ? webContents.fromId(id) : undefined;
  if (!guest || guest.isDestroyed() || guest.getType() !== "webview" || guest.hostWebContents !== sender) throw new Error("That page is gone.");
  return guest;
}

type Bounds = { x: number; y: number; width: number; height: number };

/** Each tab's DevTools: its view, and the window holding it. */
const fronts = new Map<number, { view: WebContentsView; window: BrowserWindow }>();

function placeFront(guestId: number, bounds: Bounds | null): void {
  const front = fronts.get(guestId);
  if (!front) return;
  front.view.setVisible(bounds !== null);
  if (bounds) front.view.setBounds({ x: Math.round(bounds.x), y: Math.round(bounds.y), width: Math.round(bounds.width), height: Math.round(bounds.height) });
}

function dropFront(guestId: number): void {
  const front = fronts.get(guestId);
  if (!front) return;
  fronts.delete(guestId);
  if (!front.window.isDestroyed()) front.window.contentView.removeChildView(front.view);
  if (!front.view.webContents.isDestroyed()) front.view.webContents.close();
}

async function openDevTools(sender: WebContents, guest: WebContents, bounds: Bounds): Promise<void> {
  if (fronts.has(guest.id)) return placeFront(guest.id, bounds);
  const window = BrowserWindow.fromWebContents(sender);
  if (!window) throw new Error("That window is gone.");
  const view = new WebContentsView({ webPreferences: { partition: FRONT_PARTITION, sandbox: true, contextIsolation: true, nodeIntegration: false } });
  // A front end stays on DevTools: whatever it opens comes through `devtools-open-url` on its tab's guest.
  view.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  view.webContents.on("will-navigate", (event) => {
    if (!event.url.startsWith("devtools:")) event.preventDefault();
  });
  fronts.set(guest.id, { view, window });
  placeFront(guest.id, bounds);
  window.contentView.addChildView(view);
  await beforeDevTools(guest);
  view.webContents.once("destroyed", () => devToolsEnded(guest));
  guest.once("destroyed", () => dropFront(guest.id));
  guest.setDevToolsWebContents(view.webContents);
  guest.openDevTools({ mode: "detach", activate: false });
}

type Device = { width: number; height: number; pixelRatio: number; userAgent: string | null; scale: number };

/** What each emulated tab was told, and its own user agent to go back to. */
const devices = new Map<number, { device: Device; ownAgent: string }>();

function applyDevice(guest: WebContents, device: Device): void {
  guest.enableDeviceEmulation({
    // Not "mobile", also for a phone: that lays a page out 980 wide, ignoring its width=device-width (seen on 393 and 768).
    screenPosition: "desktop",
    screenSize: { width: device.width, height: device.height },
    viewPosition: { x: 0, y: 0 },
    deviceScaleFactor: device.pixelRatio,
    viewSize: { width: device.width, height: device.height },
    scale: device.scale,
  });
}

/** The tab's page as `device`. Answers whether its user agent changed, which a page learns only as it loads. */
function setDevice(guest: WebContents, device: Device): boolean {
  let kept = devices.get(guest.id);
  if (!kept) {
    kept = { device, ownAgent: guest.getUserAgent() };
    devices.set(guest.id, kept);
    // Emulation belongs to a page's widget: a load that swaps the widget drops it, so each document gets it again.
    guest.on("did-start-navigation", (details) => {
      const now = devices.get(guest.id);
      if (now && details.isMainFrame) applyDevice(guest, now.device);
    });
    guest.on("dom-ready", () => {
      const now = devices.get(guest.id);
      if (now) applyDevice(guest, now.device);
    });
    guest.once("destroyed", () => devices.delete(guest.id));
  }
  kept.device = device;
  const agent = device.userAgent ?? kept.ownAgent;
  const changed = guest.getUserAgent() !== agent;
  if (changed) guest.setUserAgent(agent);
  applyDevice(guest, device);
  return changed;
}

function clearDevice(guest: WebContents): boolean {
  const kept = devices.get(guest.id);
  if (!kept) return false;
  devices.delete(guest.id);
  guest.disableDeviceEmulation();
  const changed = guest.getUserAgent() !== kept.ownAgent;
  if (changed) guest.setUserAgent(kept.ownAgent);
  return changed;
}

/** The picks under way, ended by Stop. */
const picking = new Map<number, () => void>();

/** Waits for the user's click on an element of the page, with the highlight Chromium draws; null when they leave. */
function pick(guest: WebContents): Promise<unknown> {
  // L128: the picker needs the debugger and never closes DevTools. An opening one counts already.
  if (devToolsOpen(guest) || fronts.has(guest.id)) return Promise.reject(new Error("devtools_open"));
  if (picking.has(guest.id)) return Promise.reject(new Error("Already picking on this page."));
  const { debugger: protocol } = guest;
  protocol.attach("1.3");
  return new Promise((resolve, reject) => {
    const finish = (end: () => void) => {
      picking.delete(guest.id);
      protocol.removeListener("message", onMessage);
      protocol.removeListener("detach", onDetach);
      guest.removeListener("before-input-event", onKey);
      if (!guest.isDestroyed() && protocol.isAttached()) protocol.detach();
      end();
    };
    const onMessage = (_event: unknown, method: string, params: { backendNodeId?: number }) => {
      if (method !== "Overlay.inspectNodeRequested" || params.backendNodeId === undefined) return;
      describe(guest, params.backendNodeId).then((picked) => finish(() => resolve(picked)), (error) => finish(() => reject(error)));
    };
    const onDetach = () => finish(() => (devToolsOpen(guest) ? reject(new Error("devtools_open")) : resolve(null)));
    const onKey = (event: Electron.Event, input: Electron.Input) => {
      if (input.type !== "keyDown" || input.key !== "Escape") return;
      event.preventDefault();
      finish(() => resolve(null));
    };
    picking.set(guest.id, () => finish(() => resolve(null)));
    protocol.on("message", onMessage);
    protocol.on("detach", onDetach);
    guest.on("before-input-event", onKey);
    const send = (method: string, params?: object) => protocol.sendCommand(method, params);
    send("DOM.enable")
      .then(() => send("Overlay.enable"))
      .then(() => send("Overlay.setInspectMode", {
        mode: "searchForNode",
        highlightConfig: { showInfo: true, showStyles: true, contentColor: { r: 111, g: 168, b: 220, a: 0.45 }, paddingColor: { r: 147, g: 196, b: 125, a: 0.4 }, marginColor: { r: 246, g: 178, b: 107, a: 0.4 } },
      }))
      .catch((error: unknown) => finish(() => reject(error)));
  });
}

/** The picked node through the agent's own script, which leaves out what fields hold, and section 3's scrubbing after it. */
async function describe(guest: WebContents, backendNodeId: number): Promise<unknown> {
  const send = (method: string, params: object) => guest.debugger.sendCommand(method, params);
  const { object } = await send("DOM.resolveNode", { backendNodeId }) as { object: { objectId: string } };
  const functionDeclaration = `function () { return (${agentScriptBody})({ kind: "describe", element: this.nodeType === 1 ? this : this.parentElement }); }`;
  const { result, exceptionDetails } = await send("Runtime.callFunctionOn", { objectId: object.objectId, functionDeclaration, returnByValue: true }) as { result: { value: { error?: string } }; exceptionDetails?: { text: string } };
  if (exceptionDetails) throw new Error(exceptionDetails.text);
  if (result.value.error) throw new Error(result.value.error);
  return sweptAndScrubbed(guest, () => result.value);
}

type Args = { guest: number; bounds?: Bounds | null; device?: Device };

export function installBrowserDevtools(): void {
  handleFromApp("brigadier:devtools", (event: IpcMainInvokeEvent, op: string, args: Args) => {
    const guest = guestFor(event.sender, args.guest);
    switch (op) {
      case "open":
        return openDevTools(event.sender, guest, args.bounds as Bounds);
      case "place":
        placeFront(guest.id, args.bounds ?? null);
        return null;
      case "close":
        guest.closeDevTools();
        dropFront(guest.id);
        return null;
      case "device":
        return setDevice(guest, args.device as Device);
      case "deviceClear":
        return clearDevice(guest);
      case "pick":
        return pick(guest);
      case "pickStop":
        picking.get(guest.id)?.();
        return null;
      default:
        throw new Error(`Unknown DevTools call ${op}`);
    }
  });
}

/** A link DevTools opens goes where a page's popup goes: a tab of the session. */
export function watchDevToolsLinks(guest: WebContents): void {
  guest.on("devtools-open-url", (_event, url) => {
    if (/^https?:/i.test(url)) guest.hostWebContents?.send("brigadier:guest-popup", { from: guest.id, url });
  });
}
