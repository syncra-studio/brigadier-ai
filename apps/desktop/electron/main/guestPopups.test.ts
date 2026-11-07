import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import test from "node:test";

import { fake } from "../testing/fakeElectron.ts";

(globalThis as { BRIGADIER_PRODUCTION?: boolean }).BRIGADIER_PRODUCTION = false;
const { installGuest } = await import("./guestPopups.ts");

type Open = { action: string; overrideBrowserWindowOptions?: { backgroundColor?: string; parent?: unknown; webPreferences?: Record<string, unknown> } };

let nextId = 500;
const sent: { channel: string; details: Record<string, unknown> }[] = [];
const host = { send: (channel: string, details: unknown) => void sent.push({ channel, details: details as Record<string, unknown> }) };

function guest(url: string, hostWebContents: typeof host | null = null) {
  let opens: (details: unknown) => Open = () => ({ action: "none" });
  const page = Object.assign(new EventEmitter(), {
    id: nextId++,
    url,
    ipc: new EventEmitter(),
    hostWebContents,
    getURL: () => page.url,
    getTitle: () => "",
    isDestroyed: () => false,
    setWindowOpenHandler: (handler: typeof opens) => { opens = handler; },
    open: (details: unknown) => opens(details),
  });
  return page;
}

function popupOf(opener: ReturnType<typeof guest>, url: string) {
  const page = guest(url);
  const window = Object.assign(new EventEmitter(), { webContents: page, setTitle: () => {} });
  opener.emit("did-create-window", window);
  return page;
}

const signInPopup = { url: "https://accounts.google.com/o/oauth2/auth", disposition: "new-window", features: "width=500,height=600" };

test("a popup paints white behind a page with no background of its own, not the app's dark window colour", () => {
  const tab = guest("https://shop.example/", host);
  installGuest(tab as never);
  const opened = tab.open(signInPopup);
  assert.equal(opened.action, "allow");
  assert.equal(opened.overrideBrowserWindowOptions?.backgroundColor, "#ffffff");
});

test("a popup's page is as locked down as a tab's, and the passkey preload reaches its frames", () => {
  const tab = guest("https://shop.example/", host);
  installGuest(tab as never);
  assert.deepEqual(tab.open(signInPopup).overrideBrowserWindowOptions?.webPreferences, {
    sandbox: true, contextIsolation: true, nodeIntegration: false, nodeIntegrationInSubFrames: true, webviewTag: false, webSecurity: true, allowRunningInsecureContent: false,
  });
});

test("only window.open with features and its opener is a window: anything else is the tab's", () => {
  sent.length = 0;
  const tab = guest("https://shop.example/", host);
  installGuest(tab as never);
  for (const features of ["", "noopener,width=500", "width=500,noreferrer"]) {
    assert.equal(tab.open({ ...signInPopup, features }).action, "deny", features);
  }
  assert.equal(tab.open({ ...signInPopup, disposition: "foreground-tab" }).action, "deny");
  assert.equal(sent.length, 4);
  assert.deepEqual(sent[0], { channel: "brigadier:guest-popup", details: { from: tab.id, url: signInPopup.url } });
});

test("a popup's refusals reach its tab's banners, however many popups deep", () => {
  sent.length = 0;
  const tab = guest("https://shop.example/", host);
  installGuest(tab as never);
  const popup = popupOf(tab, "https://auth.example/");
  const inner = popupOf(popup, "https://accounts.google.com/v3/signin/rejected");

  popup.ipc.emit("brigadier:guest-passkey-refused", { senderFrame: { url: "https://auth.example/frame" } });
  inner.ipc.emit("brigadier:guest-passkey-refused", { senderFrame: null });
  const passkey = { channel: "brigadier:guest-passkey", details: { from: tab.id } };
  assert.deepEqual(sent, [passkey, passkey]);

  inner.emit("did-finish-load");
  assert.deepEqual(sent[2], { channel: "brigadier:guest-sign-in", details: { from: tab.id, rejected: true } });
});

test("a popup opens above the app's window, when there is one", () => {
  const tab = guest("https://shop.example/", host);
  installGuest(tab as never);
  const parent = { name: "the app's window" };
  fake.hostWindow = parent;
  assert.equal(tab.open(signInPopup).overrideBrowserWindowOptions?.parent, parent);
  fake.hostWindow = null;
});
