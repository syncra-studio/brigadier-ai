import assert from "node:assert/strict";
import { test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";
import type { BrowserEvent } from "@/ipc/generated";
import { findState, openFind, useBrowserFind } from "./browserFind";
import { closePage, faviconFits, openPage } from "./browsers";

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

test("Escape from a native page closes only that tab's find bar and keeps its query", async () => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: { crypto: globalThis.crypto } });
  const channels = new Map<string, Channel<BrowserEvent>>();
  const cleared: string[] = [];
  mockIPC((command, payload) => {
    const args = payload as { id: string; events?: Channel<BrowserEvent>; query?: string };
    if (command === "browser_open") channels.set(args.id, args.events!);
    if (command === "browser_find" && args.query === "") cleared.push(args.id);
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
  } finally {
    closePage("first");
    closePage("second");
  }
});
