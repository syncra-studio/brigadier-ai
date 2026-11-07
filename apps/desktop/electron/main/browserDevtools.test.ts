import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import test from "node:test";

import { fake } from "../testing/fakeElectron.ts";

(globalThis as { BRIGADIER_PRODUCTION?: boolean }).BRIGADIER_PRODUCTION = false;

const { devToolsEnded, devToolsOpen, watchGuest } = await import("./browserAgent.ts");
const { installBrowserDevtools } = await import("./browserDevtools.ts");
const { rendererUrl } = await import("./identity.ts");

installBrowserDevtools();

const told: { from: number; event: string }[] = [];
const host = { id: 1, getType: () => "window", send: (_channel: string, event: { from: number; event: string }) => told.push(event) };
const fromApp = { sender: host, senderFrame: { parent: null, url: rendererUrl } };
const call = (op: string, args: object) => fake.handlers.get("brigadier:devtools")!(fromApp, op, args);

let nextId = 10;
function guestTab() {
  const emulated: { screenPosition: string; screenSize: unknown }[] = [];
  const guest = Object.assign(new EventEmitter(), {
    id: nextId++,
    getType: () => "webview",
    hostWebContents: host,
    isDestroyed: () => false,
    isDevToolsOpened: () => false,
    closeDevTools: () => {},
    setDevToolsWebContents: () => {},
    openDevTools: () => {},
    getUserAgent: () => "own agent",
    setUserAgent(this: { agent?: string }, agent: string) { this.agent = agent; },
    enableDeviceEmulation: (settings: { screenPosition: string; screenSize: unknown }) => emulated.push(settings),
    disableDeviceEmulation: () => {},
    getURL: () => "http://localhost/",
    debugger: Object.assign(new EventEmitter(), { attach: () => assert.fail("the debugger stays free"), isAttached: () => false }),
  });
  fake.contents.set(guest.id, guest);
  return { guest, emulated };
}

test("DevTools opened and ended are tracked by main, and the page hears both", () => {
  const { guest } = guestTab();
  watchGuest(guest as never);
  assert.equal(devToolsOpen(guest as never), false);
  guest.emit("devtools-opened");
  assert.equal(devToolsOpen(guest as never), true);
  assert.deepEqual(told.at(-1), { from: guest.id, event: "devtools-opened" });
  devToolsEnded(guest as never);
  assert.equal(devToolsOpen(guest as never), false);
  assert.deepEqual(told.at(-1), { from: guest.id, event: "devtools-closed" });
  guest.emit("devtools-opened");
  guest.emit("devtools-closed");
  assert.equal(devToolsOpen(guest as never), false, "its own event ends it too");
});

test("every device lays out as desktop, a phone too, and each new page gets it again", () => {
  const { guest, emulated } = guestTab();
  const phone = { width: 393, height: 852, pixelRatio: 3, userAgent: "iPhone", scale: 0.5 };
  assert.equal(call("device", { guest: guest.id, device: phone }), true);
  assert.equal(emulated.length, 1);
  assert.equal(emulated[0]!.screenPosition, "desktop");
  assert.deepEqual(emulated[0]!.screenSize, { width: 393, height: 852 });
  guest.emit("dom-ready");
  guest.emit("did-start-navigation", { isMainFrame: true });
  guest.emit("did-start-navigation", { isMainFrame: false });
  assert.equal(emulated.length, 3);
  assert.ok(emulated.every((each) => each.screenPosition === "desktop"));
  assert.equal(call("deviceClear", { guest: guest.id }), false, "the fake's agent never changed");
  guest.emit("dom-ready");
  assert.equal(emulated.length, 3, "left, it isn't applied again");
});

test("the picker refuses while DevTools is open, and never takes the debugger", async () => {
  const { guest } = guestTab();
  watchGuest(guest as never);
  guest.emit("devtools-opened");
  await assert.rejects(Promise.resolve().then(() => call("pick", { guest: guest.id })), /devtools_open/);
  devToolsEnded(guest as never);
});

test("the picker refuses as soon as DevTools' pane exists, before it has opened", async () => {
  const { guest } = guestTab();
  const children: unknown[] = [];
  fake.hostWindow = { isDestroyed: () => false, contentView: { addChildView: (view: unknown) => children.push(view), removeChildView: () => {} } };
  await call("open", { guest: guest.id, bounds: { x: 1, y: 2, width: 300, height: 400 } });
  assert.equal(children.length, 1);
  assert.deepEqual((children[0] as { bounds: unknown }).bounds, { x: 1, y: 2, width: 300, height: 400 });
  await assert.rejects(Promise.resolve().then(() => call("pick", { guest: guest.id })), /devtools_open/);
  call("place", { guest: guest.id, bounds: null });
  assert.equal((children[0] as { visible: boolean }).visible, false);
  call("close", { guest: guest.id });
});
