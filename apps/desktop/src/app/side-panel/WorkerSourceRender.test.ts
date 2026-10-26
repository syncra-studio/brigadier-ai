import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

/** The fixture's JSON as the page's DOM escaped it. */
const unescape = (dump: string) => dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&");

test("a worker's +N −N opens Source on its changes, read only; the switch goes back to the session's and on to another worker", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(unescape(await renderFixturePage(t, "worker-source.html", "worker-source-result", 8000))) as Record<string, unknown>;
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  // Before: the session's own checkout, as Source showed it, the switch over it.
  assert.deepEqual(seen.start, { scope: "session", switch: "This session", groups: ["Changes1"] });

  // (a) The row still opens the worker's panel; its +N −N opens the right sidebar's Source on the worker.
  assert.deepEqual(seen.rowOpens, ["t1"]);
  assert.deepEqual(seen.counts, [
    ["+12 −3", "Show Cart fixes's changes in Source"],
    ["+4 −0", "Show Readme fixes's changes in Source"],
  ]);
  assert.deepEqual(seen.opened, {
    pane: { open: true, tab: "source" },
    asked: ["t1"],
    switch: "Cart fixes",
    branch: "brigadier/w/t1into brigadier/session",
    groups: ["Changes2"],
    files: ["total.tssrc/cart+10 −3M", "loose.tssrc/cart+2 −0U"],
  });

  // (c) Read only: no message box, no Commit, More Actions, stage or discard; a file opens its diff.
  assert.deepEqual(seen.readOnly, { box: false, buttons: ["Refresh", "Open Changes"] });
  assert.deepEqual(seen.review, {
    kind: "review",
    id: "review",
    target: { type: "worker", taskId: "t1", path: "src/cart/total.ts", staged: false },
    opener: "chat",
  });

  // (b) The switch: the session, then each worker with a worktree, with its +N −N.
  assert.deepEqual(seen.menu, ["This session", "Cart fixes+12 −3", "Readme fixes+4 −0"]);
  assert.deepEqual(seen.parent, { switch: "This session", groups: ["Changes1"], box: true });
  assert.deepEqual(seen.other, { switch: "Readme fixes", files: ["README.md+4 −0M"] });
});
