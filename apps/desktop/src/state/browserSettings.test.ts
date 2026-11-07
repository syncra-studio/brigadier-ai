import assert from "node:assert/strict";
import fs from "node:fs";
import { test } from "node:test";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
  get length() { return data.size; },
  key: (at: number) => [...data.keys()][at] ?? null,
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage, crypto: globalThis.crypto, innerWidth: 1200, innerHeight: 800 } });
const { openBrowserSettings } = await import("./browserSettings");
const { useApp } = await import("./store");

test("a data list opened from a tab is that tab's profile's; opened from nowhere it's the default one", () => {
  openBrowserSettings("downloads", "work");
  assert.deepEqual(useApp.getState().selection, { type: "settings", page: "browser", detail: { view: "downloads", profile: "work" } });
  openBrowserSettings("history", "work");
  assert.deepEqual(useApp.getState().selection, { type: "settings", page: "browser", detail: { view: "history", profile: "work" } });
  openBrowserSettings("downloads");
  assert.deepEqual(useApp.getState().selection, { type: "settings", page: "browser", detail: { view: "downloads" } });
});

test("the toolbar's All downloads opens the front tab's profile", () => {
  // Seen: on a Work tab it listed the default profile's downloads ("No downloads yet").
  const source = fs.readFileSync(new URL("../app/side-panel/BrowserToolbarPopovers.tsx", import.meta.url), "utf8");
  assert.match(source, /const profile = useFrontProfile\(\)\.id;/);
  assert.match(source, /openBrowserSettings\("downloads", profile\)/);
});
