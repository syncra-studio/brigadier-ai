import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

/** The fixture's JSON as the page's DOM escaped it. */
const unescape = (dump: string) => dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&");

test("a session's Source stacks its checkout and its workers, their files right under them, read only and shown at once on coming back; a gone worker's row goes quietly", { timeout: 60000 }, async (t) => {
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

  // (a) The row still opens the worker's panel; its +N −N is plain text inside it.
  assert.deepEqual(seen.rowOpens, ["t1"]);
  assert.deepEqual(seen.counts, [["+12 −3", "SPAN", true], ["+4 −0", "SPAN", true]]);
  // Folded, its file count on its row; open, its files directly under it, no group, smaller +N −N.
  assert.deepEqual(seen.folded, { count: "2", files: [] });
  assert.deepEqual(seen.opened, {
    asked: ["t1", "t2"],
    branch: "brigadier/w/t1",
    branchTitle: "brigadier/w/t1 into brigadier/session",
    scope: "worker",
    groups: [],
    files: ["total.tssrc/cart+10 −3M", "loose.tssrc/cart+2 −0U"],
    countSize: 11,
  });

  // Coming back to the tab with the read still slow: its files on the first frame, read again behind them.
  assert.deepEqual(seen.remount, {
    unmounted: true,
    firstFrame: ["total.tssrc/cart+10 −3M", "loose.tssrc/cart+2 −0U"],
    reread: 1,
    after: ["total.tssrc/cart+10 −3M"],
  });
  // Focus twice during one read asks once; Refresh always reads.
  assert.deepEqual(seen.sharing, { asked: 1, refreshed: 1 });

  // (c) Read only: no message box, no Commit, More Actions, stage or discard; a file opens its diff.
  assert.deepEqual(seen.readOnly, { box: false, buttons: ["Open Changes", "Refresh"] });
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
