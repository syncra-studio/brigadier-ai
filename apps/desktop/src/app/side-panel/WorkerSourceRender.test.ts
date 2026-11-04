import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

/** The fixture's JSON as the page's DOM escaped it. */
const unescape = (dump: string) => dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&");

test("a session's Source stacks its checkout and its workers; a worker's +N −N opens its row, read only; a gone worker's row goes quietly", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(unescape(await renderFixturePage(t, "worker-source.html", "worker-source-result", 8000))) as Record<string, unknown>;
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  // Before: the session's own checkout open, each worker's row folded, no switch, no actions on the title.
  assert.deepEqual(seen.start, {
    rows: ["This session", "Cart fixes", "Readme fixes"],
    groups: ["Changes1"],
    folded: [[], []],
    title: "SOURCE",
    titleActions: 0,
    switcher: false,
  });

  // (a) The row still opens the worker's panel; its +N −N opens the right sidebar's Source on the worker.
  assert.deepEqual(seen.rowOpens, ["t1"]);
  assert.deepEqual(seen.counts, [
    ["+12 −3", "Show Cart fixes's changes in Source"],
    ["+4 −0", "Show Readme fixes's changes in Source"],
  ]);
  assert.deepEqual(seen.opened, {
    pane: { open: true, tab: "source" },
    asked: ["t1", "t2"],
    branch: "brigadier/w/t1",
    branchTitle: "brigadier/w/t1 into brigadier/session",
    scope: "worker",
    groups: ["Changes2"],
    files: ["total.tssrc/cart+10 −3M", "loose.tssrc/cart+2 −0U"],
    request: null,
  });

  // (c) Read only: no message box, no Commit, More Actions, stage or discard; a file opens its diff.
  assert.deepEqual(seen.readOnly, { box: false, buttons: ["Refresh", "Open Changes"] });
  assert.deepEqual(seen.review, {
    kind: "review",
    id: "review",
    target: { type: "worker", taskId: "t1", path: "src/cart/total.ts", staged: false },
    opener: "chat",
  });

  assert.deepEqual(seen.parent, { box: true });
  assert.deepEqual(seen.other, { files: ["README.md+4 −0M"] });

  // A worker that is gone closes its Review tab and loses its row, with no error; none left, no rows.
  assert.deepEqual(seen.reviewGone, { active: "chat", error: "" });
  assert.deepEqual(seen.sourceGone, { rows: ["This session", "Cart fixes"], error: "" });
  assert.deepEqual(seen.noRows, { rows: [], branch: "brigadier/session" });
});
