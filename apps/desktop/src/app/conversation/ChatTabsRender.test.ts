import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("a Chat whose agent opens a Browser tab shows it in the session tab bar, where it comes to the front and closes; the bar goes with the last tab (issue 361)", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "chat-tabs.html", "chat-tabs-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen.before, { bar: false }, "a Chat without tabs keeps its own top bar");
  assert.deepEqual(seen.opened, [{ selected: true, id: "chat" }, { selected: false, id: "browser" }], "the agent's tab opens behind the Chat");
  assert.deepEqual(seen.front, { tabs: [{ selected: false, id: "chat" }, { selected: true, id: "browser" }], panel: true, thread: "hidden" });
  assert.deepEqual(seen.closed, { tabs: 0, active: true });
});
