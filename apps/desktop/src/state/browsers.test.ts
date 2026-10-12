import assert from "node:assert/strict";
import { test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";
import type { BrowserEvent } from "@/ipc/generated";
import { findState, openFind, useBrowserFind } from "./browserFind";
import { closePage, faviconFits, openPage, useBrowsers } from "./browsers";

test("favicons follow same-document navigations but not other pages", () => {
  const loaded = { url: "https://example.com/app?x=1", loading: false };
  const loading = { ...loaded, loading: true };
  assert.equal(faviconFits(loaded, "https://example.com/app?x=1"), true);
  assert.equal(faviconFits(loading, "https://example.com/app?x=1#section"), true);
  assert.equal(faviconFits(loaded, "https://example.com/app/inbox"), true);
  assert.equal(faviconFits(loading, "https://example.com/previous"), false);
  assert.equal(faviconFits(loaded, "https://other.example/app?x=1"), false);
  assert.equal(faviconFits(loaded, "not a url"), false);
  assert.equal(faviconFits(undefined, "https://example.com/app?x=1"), false);
});

test("native page keys affect only their tab: Escape keeps the find query, shifted + zooms 10%", async () => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: { crypto: globalThis.crypto } });
  const channels = new Map<string, Channel<BrowserEvent>>();
  const cleared: string[] = [];
  const zoomed: { id: string; zoom: number }[] = [];
  mockIPC((command, payload) => {
    const args = payload as { id: string; events?: Channel<BrowserEvent>; query?: string; zoom: number };
    if (command === "browser_open") channels.set(args.id, args.events!);
    if (command === "browser_find" && args.query === "") cleared.push(args.id);
    if (command === "browser_zoom") {
      zoomed.push({ id: args.id, zoom: args.zoom });
      return args.zoom;
    }
    return null;
  });
  try {
    for (const id of ["first", "second"]) {
      await openPage(id, "https://example.com/", { x: 0, y: 0, width: 800, height: 600 });
      openFind(id);
    }
    useBrowserFind.setState(({ tabs }) => ({ tabs: { ...tabs, first: { ...tabs.first!, query: "alpha", found: true, current: 1, total: 3 } } }));
    channels.get("first")!.onmessage({ type: "escape" });
    assert.equal(findState("first").open, false);
    assert.equal(findState("first").query, "alpha");
    assert.equal(findState("first").found, false);
    assert.equal(findState("second").open, true);
    assert.deepEqual(cleared, ["first"]);
    channels.get("first")!.onmessage({ type: "escape" });
    assert.deepEqual(cleared, ["first"], "Escape with no find bar does no work");
    channels.get("first")!.onmessage({ type: "zoomIn" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    channels.get("first")!.onmessage({ type: "zoomIn" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(zoomed, [{ id: "first", zoom: 1.1 }, { id: "first", zoom: 1.2 }]);
    assert.equal(useBrowsers.getState().pages.first?.zoom, 1.2);
    assert.equal(useBrowsers.getState().pages.second?.zoom, undefined);
  } finally {
    closePage("first");
    closePage("second");
  }
});
