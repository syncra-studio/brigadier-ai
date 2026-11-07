import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import test from "node:test";

import "../testing/fakeElectron.ts";

const { stopOf, watchPasskeyRefusals, watchSignIn } = await import("./browserSignIn.ts");

type Frame = { url: string; isDestroyed: () => boolean; executeJavaScript: (code: string) => Promise<unknown> };

const frame = (url: string, answer: (code: string) => Promise<unknown> = async () => false): Frame => ({ url, isDestroyed: () => false, executeJavaScript: answer });
const AGENT_WORLD = 1010;
const LOOK_MS = 250;
const later = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

let nextId = 100;

/** A guest or a popup's page. `fields`: whether the page's own document shows a password or code field. */
function contents(url: string, { frames = [] as Frame[], fields = false as boolean | Error, host = null as { send: (channel: string, details: unknown) => void } | null } = {}) {
  const page = Object.assign(new EventEmitter(), {
    id: nextId++,
    url,
    destroyed: false,
    worlds: [] as number[],
    codes: [] as string[],
    ipc: new EventEmitter(),
    hostWebContents: host,
    getURL: () => page.url,
    isDestroyed: () => page.destroyed,
    mainFrame: {} as { framesInSubtree: unknown[] },
    executeJavaScriptInIsolatedWorld: async (world: number, scripts: { code: string }[]) => {
      page.worlds.push(world);
      page.codes.push(scripts[0]!.code);
      if (fields instanceof Error) throw fields;
      return fields;
    },
  });
  page.mainFrame = { framesInSubtree: [page.mainFrame, ...frames] };
  page.mainFrame.framesInSubtree[0] = page.mainFrame;
  return page;
}

function hostOf() {
  const sent: { channel: string; details: Record<string, unknown> }[] = [];
  return { sent, send: (channel: string, details: unknown) => void sent.push({ channel, details: details as Record<string, unknown> }) };
}

const stop = (page: ReturnType<typeof contents>) => stopOf(page as never);
const watched = (page: ReturnType<typeof contents>, tab = page) => watchSignIn(page as never, tab as never);

test("a sign-in by address needs no look into the page", async () => {
  const page = contents("https://accounts.google.com/v3/signin/identifier");
  assert.equal((await stop(page))?.kind, "signIn");
  assert.deepEqual(page.worlds, []);
});

test("a password field stops the agent, looked for in the agent's world, where the page can't reach", async () => {
  const page = contents("https://shop.example/account", { fields: true });
  assert.deepEqual(await stop(page), { kind: "signIn", site: "shop.example", key: "signIn https://shop.example" });
  assert.deepEqual(page.worlds, [AGENT_WORLD]);
});

test("a password field in a frame stops it too, on the page's own site", async () => {
  const page = contents("https://shop.example/", { frames: [frame("https://auth.example/embed", async () => true)] });
  assert.deepEqual(await stop(page), { kind: "signIn", site: "shop.example", key: "signIn https://shop.example" });
});

test("a page with nothing to answer stops nothing, whatever its frames and its own document do", async () => {
  assert.equal(await stop(contents("https://shop.example/", { frames: [frame("https://ads.example/")] })), null);
  assert.equal(await stop(contents("https://shop.example/", { fields: new Error("page is navigating") })), null);
  assert.equal(await stop(contents("https://shop.example/", { frames: [frame("https://ads.example/", () => Promise.reject(new Error("gone")))] })), null);
});

test("a frame that never answers doesn't hold the agent up for long", async () => {
  const started = Date.now();
  assert.equal(await stop(contents("https://shop.example/", { frames: [frame("https://stuck.example/", () => new Promise(() => {}))] })), null);
  assert.ok(Date.now() - started < 3000);
});

test("a destroyed page and a destroyed frame are nobody's stop", async () => {
  const gone = contents("https://accounts.google.com/");
  gone.destroyed = true;
  assert.equal(await stop(gone), null);
  const dead = { ...frame("https://auth.example/", async () => true), isDestroyed: () => true };
  assert.equal(await stop(contents("https://shop.example/", { frames: [dead] })), null);
});

test("a captcha stops at its page's own address, if its frame takes up room", async () => {
  const anchor = "https://www.google.com/recaptcha/api2/anchor?k=key";
  assert.deepEqual(await stop(contents("https://shop.example/cart?step=2", { frames: [frame(anchor, async () => true)] })), { kind: "captcha", site: "shop.example", key: "captcha https://shop.example/cart" });
  // A widget of no size is there on many pages and asks nobody anything.
  assert.equal(await stop(contents("https://shop.example/cart", { frames: [frame(anchor, async () => false)] })), null);
});

/** Runs the page's look for fields over a small document, as the page's world would. */
async function fieldsOver(inputs: Record<string, unknown>[], shadow: Record<string, unknown>[] = []) {
  const page = contents("https://shop.example/", { fields: false });
  await stop(page);
  const input = (each: Record<string, unknown>) => ({ type: "text", autocomplete: "", name: "", id: "", width: 100, height: 20, right: 100, bottom: 20, visibility: "visible", display: "inline", opacity: "1", ...each });
  const root = (fields: Record<string, unknown>[], hosts: unknown[] = []) => ({
    querySelectorAll: (selector: string) => selector === "input" ? fields.map((each) => {
      const seen = input(each);
      return { ...seen, getBoundingClientRect: () => seen };
    }) : hosts,
  });
  const globals = globalThis as { document?: unknown; getComputedStyle?: unknown };
  const had = { document: globals.document, getComputedStyle: globals.getComputedStyle };
  globals.document = root(inputs, shadow.length ? [{ shadowRoot: root(shadow) }] : []);
  globals.getComputedStyle = (each: unknown) => each;
  try {
    return (0, eval)(page.codes[0]!) as boolean;
  } finally {
    Object.assign(globals, had);
  }
}

test("what counts as a field that asks: shown, for a password or a one-time code", async () => {
  assert.equal(await fieldsOver([{ type: "password" }]), true);
  assert.equal(await fieldsOver([{ autocomplete: "one-time-code" }]), true);
  assert.equal(await fieldsOver([{ name: "otp" }]), true);
  assert.equal(await fieldsOver([{ id: "two-factor" }]), true);
  assert.equal(await fieldsOver([{ type: "search", name: "q" }]), false);
  assert.equal(await fieldsOver([{ type: "password", visibility: "hidden" }]), false);
  assert.equal(await fieldsOver([{ type: "password", display: "none" }]), false);
  assert.equal(await fieldsOver([{ type: "password", opacity: "0" }]), false);
  assert.equal(await fieldsOver([{ type: "password", width: 1, height: 1 }]), false, "a browser-autofill trap of no size");
  assert.equal(await fieldsOver([], [{ type: "password" }]), true, "in an open shadow root");
});

test("Google's refusal is told once to the page hosting the tab, and again when the page leaves it", () => {
  const host = hostOf();
  const tab = contents("https://accounts.google.com/v3/signin/rejected", { host });
  watched(tab);
  tab.emit("did-finish-load");
  tab.emit("did-finish-load");
  assert.deepEqual(host.sent, [{ channel: "brigadier:guest-sign-in", details: { from: tab.id, rejected: true } }]);
  tab.url = "https://accounts.google.com/v3/signin/identifier";
  tab.emit("did-navigate-in-page");
  assert.deepEqual(host.sent[1], { channel: "brigadier:guest-sign-in", details: { from: tab.id, rejected: false } });
  assert.equal(host.sent.length, 2);
});

test("a tab's first load isn't told at did-navigate, when the app's page can't tell which tab it is yet", () => {
  const host = hostOf();
  const tab = contents("https://accounts.google.com/signin/rejected", { host });
  watched(tab);
  tab.emit("did-navigate");
  assert.deepEqual(host.sent, []);
  tab.emit("did-finish-load");
  assert.equal(host.sent.length, 1);
});

test("a popup's refusal reaches its tab's banner, by the tab's id", () => {
  const host = hostOf();
  const tab = contents("https://shop.example/", { host });
  const popup = contents("https://accounts.google.com/v3/signin/rejected");
  watched(popup, tab);
  popup.emit("did-finish-load");
  assert.deepEqual(host.sent, [{ channel: "brigadier:guest-sign-in", details: { from: tab.id, rejected: true } }]);
});

test("a closed tab hears nothing of its popup", () => {
  const host = hostOf();
  const tab = contents("https://shop.example/", { host });
  const popup = contents("https://accounts.google.com/v3/signin/rejected");
  watched(popup, tab);
  tab.destroyed = true;
  popup.emit("did-finish-load");
  assert.deepEqual(host.sent, []);
});

test("a tab asks the renderer to look once a burst of loads is over, since only it knows whether an agent drives", async () => {
  const host = hostOf();
  const tab = contents("https://shop.example/", { host, fields: true });
  watched(tab);
  tab.emit("did-stop-loading");
  tab.emit("did-frame-finish-load");
  tab.emit("did-navigate-in-page");
  await later(LOOK_MS + 80);
  assert.deepEqual(host.sent, [{ channel: "brigadier:guest-sign-in", details: { from: tab.id, look: true } }]);
  assert.deepEqual(tab.worlds, [], "a tab's page isn't looked into until the renderer asks");
});

test("a popup is looked into at once, and a sign-in in it says it is a window", async () => {
  const host = hostOf();
  const tab = contents("https://shop.example/", { host });
  const popup = contents("https://accounts.google.com/o/oauth2/auth");
  watched(popup, tab);
  popup.emit("did-stop-loading");
  await later(LOOK_MS + 80);
  assert.deepEqual(host.sent, [{
    channel: "brigadier:guest-sign-in",
    details: { from: tab.id, stop: { kind: "signIn", site: "accounts.google.com", key: "signIn https://accounts.google.com", window: true } },
  }]);
});

test("a popup with nothing to answer says nothing, and one closed before the look isn't looked at", async () => {
  const host = hostOf();
  const tab = contents("https://shop.example/", { host });
  const plain = contents("https://shop.example/help");
  watched(plain, tab);
  plain.emit("did-stop-loading");
  const closing = contents("https://accounts.google.com/");
  watched(closing, tab);
  closing.emit("did-stop-loading");
  closing.destroyed = true;
  closing.emit("destroyed");
  await later(LOOK_MS + 80);
  assert.deepEqual(host.sent, []);
});

test("a passkey refused in any frame reaches the tab's banner, a popup's too", () => {
  const host = hostOf();
  const tab = contents("https://shop.example/", { host });
  const popup = contents("https://accounts.google.com/");
  watchPasskeyRefusals(tab as never, tab as never);
  watchPasskeyRefusals(popup as never, tab as never);
  const told = { channel: "brigadier:guest-passkey", details: { from: tab.id } };
  tab.ipc.emit("brigadier:guest-passkey-refused", { senderFrame: { url: "https://auth.example/embedded-frame" } });
  assert.deepEqual(host.sent, [told]);
  popup.ipc.emit("brigadier:guest-passkey-refused", { senderFrame: null });
  assert.deepEqual(host.sent, [told, told]);
  tab.destroyed = true;
  popup.ipc.emit("brigadier:guest-passkey-refused", { senderFrame: null });
  assert.equal(host.sent.length, 2);
});
