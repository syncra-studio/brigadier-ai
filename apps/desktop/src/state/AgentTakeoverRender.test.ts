import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The Browser tab's user-input script in Chromium: the user's click, key or scroll pauses an
// agent's tab; a page's own events and the browser tool's clicks and typing never do.
test("browser tool: only the user's own input takes a tab over", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "agent-takeover.html", "agent-takeover-result", 60000)) as Record<string, unknown>;
  const done = { done: true };
  assert.deepEqual([seen.agentClick, seen.agentType], [done, done], "the browser tool worked the page");
  assert.deepEqual(seen.scripts, [], "scripts' events and the tool's actions aren't the user's");
  const once = ["browser:user-input"];
  assert.deepEqual(seen.click, once, "a click is");
  const soon = seen.soon as { posted: string[]; within: boolean };
  // A busy machine may take longer than the second between the two.
  if (soon.within) assert.deepEqual(soon.posted, once, "at most once a second");
  const before = soon.posted.length;
  assert.equal((seen.wheel as string[]).length, before + 1, "a scroll is");
  assert.equal((seen.key as string[]).length, before + 2, "a key is");
  assert.ok((seen.key as string[]).every((message) => message === "browser:user-input"), "and nothing else is sent");
});
