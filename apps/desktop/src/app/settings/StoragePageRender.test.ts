import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real page in Chromium over a faked daemon: Refresh, Review and Free up, the result,
// choosing items by hand, disk use per project and Manage.
test("storage refreshes, reviews, frees up space and chooses items", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "storage.html?drive=1", "storage-result", 12000)) as Record<
    string,
    unknown
  >;
  assert.equal(seen.status, "31.0 GB used by Brigadier");
  assert.deepEqual(seen.legend, [
    "Work folders18.0 GB",
    "Conversations3.1 GB",
    "Brains1.5 GB",
    "Models3.0 GB",
    // The result is read from the page's markup, so its "&" stays escaped.
    "Logs &amp; recordings900.0 MB",
    "Other4.6 GB",
  ]);
  assert.equal(seen.freeRow, "Free up space11.4 GB can be freed. Review what goes before anything is removed.Review");
  assert.equal(seen.scansAtOpen, 1);
  assert.equal(seen.revealed, "open");
  assert.equal(seen.scansAfterRefresh, 2);
  assert.deepEqual(seen.review, {
    title: "Free up space",
    groups: [
      "Finished sessions’ work folders1 item9.0 GB",
      "Build files in idle sessions1 item2.0 GB",
      "Old logs1 item400.0 MB",
      "Kept on purpose3 things6.0 GB",
    ],
    footer: "3 things are kept on purpose. Nothing else is touched.",
  });
  // At the column's dialog width (32rem), not across the window.
  assert.ok(Number(seen.reviewWidth) > 0 && Number(seen.reviewWidth) <= 512, `review is ${String(seen.reviewWidth)}px wide`);
  assert.deepEqual(seen.firstGroup, ["Work folder of “Fix login”Its session ended and its branch is merged.Reveal in Finder9.0 GB"]);
  // The one button removes exactly the safe items of the latest scan.
  assert.equal(
    seen.result,
    "Freed 9.0 GB.1 item stays, as below.Build files of “Checkout page”Left in place: something is using it now.A process runs in itDoneClose",
  );
  assert.deepEqual(seen.afterDone, { dialog: false, scans: 3 });
  // By hand: the safe items start picked; the unmerged branch is never offered.
  assert.deepEqual(seen.choose, [
    { label: "Work folder of “Fix login”", checked: true },
    { label: "Work folder of “Draft”", checked: false },
    { label: "Build files of “Checkout page”", checked: true },
    { label: "Logs older than 30 days", checked: true },
    { label: "Speech model (base)", checked: false },
  ]);
  assert.equal(seen.removeButton, "Remove 4 items (13.4 GB)");
  assert.deepEqual(seen.chosenReview, {
    title: "Remove chosen items",
    groups: [
      "Finished sessions’ work folders2 items11.0 GB",
      "Build files in idle sessions1 item2.0 GB",
      "Old logs1 item400.0 MB",
      "Kept on purpose3 things6.0 GB",
    ],
    action: "Free up 13.4 GB",
  });
  // Cancel removes nothing.
  assert.deepEqual(seen.afterCancel, { dialog: false, calls: 1 });
  assert.deepEqual((seen.usage as string[][])[0], ["storefront-web", "14.0 GB", "1.0 GB", "2.0 GB", "200.0 MB", "17.2 GB"]);
  assert.deepEqual((seen.usage as string[][])[1]?.[0], "billing-api · repository not found");
  assert.equal((seen.usage as string[][]).length, 7);
  assert.deepEqual(seen.selection, { type: "settings", page: "archived" });
  // Reviewed from scan 1 while scan 2 was out: the clean names scan 1.
  assert.deepEqual(seen.calls, ["clean scan-1 done,build,logs"]);
});
