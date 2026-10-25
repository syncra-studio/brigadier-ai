import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Snapshot = { selection: unknown; title: string; index: number };
type Seen = {
  error?: string;
  detail: Snapshot; back: Snapshot; forward: Snapshot; keyForward: Snapshot;
  focusAfterBack: Snapshot & { heading: boolean };
  search: Snapshot & { focused: boolean }; searchBack: Snapshot & { focused: boolean };
  escapeSearch: unknown; escapeClose: unknown;
  browser: Snapshot[]; pluginBack: Snapshot; pluginForward: Snapshot;
  signIn: Snapshot & { count: number; pending: unknown };
  blocked: unknown; unclaimed: boolean; nativeMenu: Snapshot & { calls: string[] };
  ctrlForward: Snapshot; mouse: Snapshot & { down: boolean; held: boolean; up: boolean };
  truncated: Snapshot & { disabled: boolean };
};

// Real titlebar arrows and input focus, plus the menu bridge macOS uses while a native page
// owns the keyboard. BrowserTabRender separately exercises native browser history.
test("global arrows and keyboard restore nested Settings, preserve focus, and never replay transient requests", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "navigation.html", "navigation-result", 60000)) as Seen;
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  const list = { type: "settings", page: "providers" };
  const detail = (provider: string) => ({ ...list, detail: { provider } });
  assert.deepEqual(seen.detail.selection, detail("claude"));
  assert.deepEqual(seen.back.selection, list);
  assert.deepEqual(seen.forward.selection, detail("claude"));
  assert.deepEqual(seen.focusAfterBack.selection, list);
  assert.equal(seen.focusAfterBack.heading, true);
  assert.deepEqual(seen.keyForward.selection, detail("claude"));
  assert.deepEqual(seen.search.selection, detail("claude"));
  assert.equal(seen.search.focused, true);
  assert.deepEqual(seen.searchBack.selection, list);
  assert.equal(seen.searchBack.focused, true);
  assert.deepEqual(seen.escapeSearch, { value: "", selection: list });
  assert.deepEqual(seen.escapeClose, { type: "draft", kind: "chat" });
  assert.deepEqual(seen.browser.map((entry) => entry.title), [
    "Browsing history", "Browser", "Browsing history", "Download history", "Browser", "Download history", "Site settings", "Browser", "Site settings",
  ]);
  assert.deepEqual(seen.pluginBack.selection, { type: "settings", page: "plugins" });
  assert.equal(seen.pluginForward.title, "Example server");
  assert.deepEqual(seen.pluginForward.selection, { type: "settings", page: "plugins", detail: { plugin: "server:example" } });
  assert.equal(seen.signIn.count, 1);
  assert.equal(seen.signIn.pending, null);
  assert.deepEqual(seen.signIn.selection, detail("codex"));
  assert.deepEqual(seen.blocked, ["dialog", "alertdialog", "menu", "listbox"].map((role) => ({ role, prevented: true, stayed: true })));
  assert.equal(seen.unclaimed, true);
  assert.deepEqual(seen.nativeMenu.selection, list);
  assert.deepEqual(seen.nativeMenu.calls, ["focus app"]);
  assert.deepEqual(seen.ctrlForward.selection, detail("codex"));
  assert.deepEqual([seen.mouse.down, seen.mouse.held, seen.mouse.up, seen.mouse.selection], [true, true, true, list]);
  assert.equal(seen.truncated.title, "Git");
  assert.equal(seen.truncated.disabled, true);
});
