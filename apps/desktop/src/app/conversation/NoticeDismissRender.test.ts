import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

const ACCOUNT =
  "Your organization has disabled Claude subscription access for Claude Code · Use an Anthropic API key instead, or ask your admin to enable access";

test("a dismissed notice stays dismissed when the tray is drawn anew, and a new failure shows again (D51)", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "notice-dismiss.html?drive=1", "notice-dismiss-result", 10000)) as {
    before: string[];
    dismissed: string[];
    reread: string[];
    again: string[];
    dismissCalls: number;
  };
  assert.deepEqual(seen.before, [ACCOUNT]);
  assert.deepEqual(seen.dismissed, []);
  // The daemon keeps the dismissal: a fresh read and a new draw don't bring it back.
  assert.deepEqual(seen.reread, []);
  assert.equal(seen.dismissCalls, 1);
  // The problem happening again is a new notice, and it shows.
  assert.deepEqual(seen.again, [ACCOUNT]);
});
