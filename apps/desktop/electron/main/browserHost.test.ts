import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { fake } from "../testing/fakeElectron.ts";

(globalThis as { BRIGADIER_PRODUCTION?: boolean }).BRIGADIER_PRODUCTION = false;
fake.userData = fs.mkdtempSync(path.join(os.tmpdir(), "brigadier-host-test-"));
test.after(() => fs.rmSync(fake.userData, { recursive: true, force: true }));

const { installBrowserHost } = await import("./browserHost.ts");
const { rendererUrl } = await import("./identity.ts");
const { guestPasskeyPath } = await import("./windows.ts");

installBrowserHost();

const fire = (name: string, ...args: unknown[]) => {
  for (const listener of fake.appListeners.get(name) ?? []) (listener as (...each: unknown[]) => void)(...args);
};

const appPageCall = { sender: { id: 1, getType: () => "window" }, senderFrame: { parent: null, url: rendererUrl } };
const partition = fake.handlers.get("brigadier:profile-partition")!(appPageCall, "work") as string;

function hostPage() {
  const page = Object.assign(new EventEmitter(), { id: 1, getType: () => "window", getURL: () => rendererUrl, session: {} });
  fire("web-contents-created", {}, page);
  return page;
}

test("a tab's guest lets the passkey preload into its frames, with no Node in any of them", () => {
  const preferences: Record<string, unknown> = { preload: "/somewhere/else.js", nodeIntegration: true, nodeIntegrationInSubFrames: false, sandbox: false, contextIsolation: false, webSecurity: false, allowRunningInsecureContent: true };
  let refused = false;
  hostPage().emit("will-attach-webview", { preventDefault: () => { refused = true; } }, preferences, { partition, src: "https://shop.example/" });
  assert.equal(refused, false);
  // Electron runs a session's preloads in subframes only with this on, and a passkey request there hung forever.
  assert.equal(preferences.nodeIntegrationInSubFrames, true);
  assert.deepEqual(preferences, { nodeIntegration: false, nodeIntegrationInSubFrames: true, contextIsolation: true, sandbox: true, webSecurity: true, allowRunningInsecureContent: false });
});

test("a guest on a session that isn't a profile's, or at an address that isn't the web's, isn't attached", () => {
  for (const params of [{ partition: "persist:elsewhere", src: "https://shop.example/" }, { partition, src: "file:///etc/hosts" }]) {
    let refused = false;
    hostPage().emit("will-attach-webview", { preventDefault: () => { refused = true; } }, {}, params);
    assert.equal(refused, true, JSON.stringify(params));
  }
});

test("a profile's session runs the passkey preload in every frame, on macOS", { skip: process.platform !== "darwin" }, () => {
  const registered: { type: string; filePath: string }[] = [];
  const created = {
    storagePath: path.join(fake.userData, "Partitions", "browser-profile-work"),
    registerPreloadScript: (script: { type: string; filePath: string }) => registered.push(script),
    setPermissionRequestHandler: () => {},
    setPermissionCheckHandler: () => {},
    setDevicePermissionHandler: () => {},
    setDisplayMediaRequestHandler: () => {},
    cookies: new EventEmitter(),
    webRequest: { onSendHeaders: () => {}, onCompleted: () => {}, onErrorOccurred: () => {} },
    on: () => {},
  };
  fire("session-created", created);
  assert.ok(registered.some((script) => script.type === "frame" && script.filePath === guestPasskeyPath), JSON.stringify(registered));
});
