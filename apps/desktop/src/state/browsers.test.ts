import assert from "node:assert/strict";
import { test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";
import type { BrowserEvent } from "@/ipc/generated";
import { findState, openFind, useBrowserFind } from "./browserFind";
import { closePage, faviconFits, openPage, useBrowsers } from "./browsers";
import { type BrowserTabState, CHAT_TAB, restoreSessionTabs, sessionTabs, useSessionTabs } from "./sessionTabs";

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

test("a page's popup opens as a tab right after its opener's, in front; it closes itself; with the opener's tab gone it goes", async () => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: { crypto: globalThis.crypto } });
  const channels = new Map<string, Channel<BrowserEvent>>();
  const closed: string[] = [];
  mockIPC((command, payload) => {
    const args = payload as { id: string; events?: Channel<BrowserEvent> };
    if (command === "browser_open") channels.set(args.id, args.events!);
    if (command === "browser_close") closed.push(args.id);
    return null;
  });
  const opener: BrowserTabState = { kind: "browser", id: "browser:opener", opener: CHAT_TAB, url: "https://example.com/", title: "", agent: true };
  const other: BrowserTabState = { kind: "browser", id: "browser:other", opener: CHAT_TAB, url: "https://example.org/", title: "" };
  useSessionTabs.setState({ sessions: { session: { tabs: [opener, other], active: other.id } } });
  try {
    await openPage(opener.id, opener.url, { x: 0, y: 0, width: 800, height: 600 });
    channels.get(opener.id)!.onmessage({ type: "popup", id: "browser:popup-1", url: "" });
    const shown = sessionTabs("session");
    assert.deepEqual(shown.tabs.map((tab) => tab.id), [opener.id, "browser:popup-1", other.id]);
    assert.equal(shown.active, "browser:popup-1");
    assert.deepEqual(shown.tabs[1], { kind: "browser", id: "browser:popup-1", opener: opener.id, url: "about:blank", title: "", agent: true });
    const restored = restoreSessionTabs({ session: shown }).session!.tabs;
    assert.deepEqual(restored.map((tab) => tab.kind === "browser" ? tab.url : null), ["https://example.com/", "", "https://example.org/"], "a written popup starts empty after a restart");
    await openPage("browser:popup-1", "about:blank", { x: 0, y: 0, width: 800, height: 600 });
    channels.get("browser:popup-1")!.onmessage({ type: "closed" });
    assert.deepEqual(sessionTabs("session").tabs.map((tab) => tab.id), [opener.id, other.id]);
    assert.equal(sessionTabs("session").active, opener.id, "closing returns to its opener");
    useSessionTabs.setState({ sessions: {} });
    channels.get(opener.id)!.onmessage({ type: "popup", id: "browser:popup-2", url: "https://example.net/" });
    assert.deepEqual(closed, ["browser:popup-2"], "a popup no tab can take is dropped");
  } finally {
    closePage(opener.id);
    closePage("browser:popup-1");
    useSessionTabs.setState({ sessions: {} });
  }
});
