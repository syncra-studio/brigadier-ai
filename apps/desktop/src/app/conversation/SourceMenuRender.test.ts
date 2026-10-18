import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

/** The fixture's JSON as the page's DOM escaped it. */
const unescape = (dump: string) => dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&");

test("the Source panel's menu makes, deletes and publishes branches, stashes, and shows Git Output (issue 257)", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(unescape(await renderFixturePage(t, "source-menu.html", "source-menu-result", 8000))) as Record<string, unknown>;
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.deepEqual(seen.moreMenu, ["Pull", "Push", "Checkout to…", "Fetch", "Commit", "Changes", "Pull, Push", "Branch", "Stash", "Show Git Output"]);
  assert.deepEqual(seen.branchMenu, [
    "Merge…",
    "Rebase Branch…",
    "Create Branch…",
    "Create Branch From…",
    "Rename Branch…",
    "Delete Branch…",
    "Publish Branch",
  ]);
  assert.deepEqual(seen.stashMenu, ["Stash", "Stash (Include Untracked)", "Apply Latest Stash", "Pop Latest Stash", "Drop Stash…"]);
  assert.deepEqual(seen.nameBox, ["New branch name", "Enter creates it from main and switches to it"]);
  // Local branches but the one checked out (topic, just created).
  assert.deepEqual(seen.deletePicker, ["Select a branch to delete", ["main", "feature/coupons"]]);
  assert.deepEqual(seen.notMerged, ["feature/coupons isn't merged yet. Delete it anyway? Its commits are lost.", ["Delete Anyway"]]);
  assert.equal(seen.dropAsked, "Drop the latest stash?");
  assert.deepEqual(seen.actions, [
    { type: "createBranch", name: "topic", from: null },
    { type: "deleteBranch", name: "feature/coupons", force: false },
    { type: "deleteBranch", name: "feature/coupons", force: true },
    { type: "dropStash" },
  ]);
  const output = seen.output as { title: string; runs: string[]; failedRed: string[]; buttons: string[] };
  assert.equal(output.title, "Git Output");
  assert.match(output.runs[0]!, /> git switch --quiet -c topic$/);
  assert.match(output.runs[1]!, /> git branch -d -- feature\/coupons/);
  assert.deepEqual(output.failedRed, ["error: the branch 'feature/coupons' is not fully merged."]);
  // Copy, and the dialog's own X (read as Close): no Close button of its own.
  assert.deepEqual(output.buttons.toSorted(), ["Close", "Copy"]);
});
