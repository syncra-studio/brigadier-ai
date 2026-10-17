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
  assert.equal(seen.freeRow, "Free up space11.4 GB can be freed; review what goes before anything is removedReview");
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
  // By hand: the safe items start picked, each group's box shows whether all of it is; the
  // unmerged branch is never offered.
  assert.equal(seen.chooseTitle, "Choose what to remove");
  // Compact: a line a group, closed, with how much of it is picked and its size.
  assert.deepEqual(seen.chooseGroups, [
    "Finished sessions’ work folders1 of 211.0 GB",
    "Build files in idle sessions1 item2.0 GB",
    "Old logs1 item400.0 MB",
    "Downloaded models0 of 1150.0 MB",
  ]);
  assert.equal(seen.chooseItemsAtOpen, 0);
  assert.ok(Number(seen.chooseHeightAtOpen) <= 400, `choose opens ${String(seen.chooseHeightAtOpen)}px tall`);
  // Every group open, it never grows past 34rem: its list scrolls.
  assert.ok(Number(seen.chooseHeight) <= 544, `choose is ${String(seen.chooseHeight)}px tall`);
  assert.equal(seen.chooseScrolls, true);
  assert.deepEqual(seen.choose, [
    { label: "Finished sessions’ work folders", state: "indeterminate" },
    { label: "Work folder of “Fix login”", state: "checked" },
    { label: "Work folder of “Draft”", state: "unchecked" },
    { label: "Build files in idle sessions", state: "checked" },
    { label: "Build files of “Checkout page”", state: "checked" },
    { label: "Old logs", state: "checked" },
    { label: "Logs older than 30 days", state: "checked" },
    { label: "Downloaded models", state: "unchecked" },
    { label: "Speech model (base)", state: "unchecked" },
  ]);
  // What each item is and what happens to it, in plain words.
  assert.deepEqual(seen.chooseNotes, [
    "Its session ended and its branch is merged.",
    "Has unsaved changes, so it isn't part of the sweep. Its changes are kept as a commit on its branch first.",
    "Rebuilt when the session runs again.",
    "Old logs.",
    "Goes to the Trash.",
  ]);
  assert.equal(seen.chooseStatus, "11.4 GB selected");
  assert.equal(seen.reveals, 1);
  // The group's box picks the rest of it.
  assert.deepEqual(seen.groupPicked, [
    { label: "Finished sessions’ work folders", state: "checked" },
    { label: "Work folder of “Fix login”", state: "checked" },
    { label: "Work folder of “Draft”", state: "checked" },
  ]);
  assert.equal(seen.chooseStatusAfter, "13.4 GB selected");
  // The one button goes to the same confirmation as Free up space.
  assert.equal(seen.removeButton, "Review 4 items");
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
  // Disk use: each project, largest first, with its total and parts; then what all share.
  assert.equal(seen.usageTitle, "Disk use per project");
  assert.deepEqual(seen.usage, [
    {
      name: "storefront-web17.2 GB",
      parts: ["Work folders 14.0 GB", "Conversations 2.0 GB", "Brain 1.0 GB", "Scratch 200.0 MB"],
    },
    { name: "billing-apiRepository not found4.8 GB", parts: ["Work folders 4.0 GB", "Conversations 500.0 MB", "Brain 300.0 MB"] },
  ]);
  assert.deepEqual(seen.shared, [
    "Database600.0 MB",
    "Downloaded models3.0 GB",
    "Logs900.0 MB",
    "Personal Brain200.0 MB",
    "Everything else1.0 GB",
  ]);
  assert.deepEqual(seen.usageButtons, ["Reveal in Finder", "Done", "Close"]);
  // Nothing found: both say so, and nothing can be reviewed.
  assert.deepEqual(seen.chooseEmpty, { text: "Nothing can be removed right now.", review: true });
  assert.equal(seen.usageEmpty, "No projects yet.");
  assert.deepEqual(seen.selection, { type: "settings", page: "archived" });
  // Reviewed from scan 1 while scan 2 was out: the clean names scan 1.
  assert.deepEqual(seen.calls, ["clean scan-1 done,build,logs"]);
});
