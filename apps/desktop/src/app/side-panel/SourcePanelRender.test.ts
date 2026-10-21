import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

/** The fixture's JSON as the page's DOM escaped it. */
const unescape = (dump: string) => dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&");

test("the Source panel commits from its menu, merges a branch in, lists its conflicts and continues once they're staged", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(unescape(await renderFixturePage(t, "source-panel.html", "source-panel-result", 8000))) as Record<string, unknown>;
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.deepEqual(seen.groups, ["Staged Changes1", "Changes2"]);
  assert.equal(seen.placeholder, 'Message (⌘⏎ to commit on "brigadier/912a9469/session")');
  // The empty box is as tall as its placeholder, which wraps in the panel (#490).
  assert.deepEqual(seen.placeholderFits, { fits: true, lines: 2 });
  assert.equal(seen.commit, "Commit");
  assert.equal(seen.branch, "brigadier/912a9469/session2");
  assert.doesNotMatch(String(seen.panelText), /Leave out AI co-author lines|Review all/);
  assert.deepEqual(seen.rowActions, [
    ["Open File", "Discard Changes", "Stage Changes"],
    ["Open File", "Discard Changes", "Stage Changes"],
  ]);
  assert.deepEqual(seen.commitMenu, ["Commit", "Commit (Amend)", "Commit & Push", "Commit & Sync"]);
  // A session works on its own branch: no Checkout to….
  assert.deepEqual(seen.moreMenu, ["Pull", "Push", "Fetch", "Commit", "Changes", "Pull, Push", "Branch", "Stash", "Show Git Output"]);
  assert.equal(seen.pickerPlaceholder, "Select a branch to merge from");
  assert.equal((seen.picker as string[]).length, 3, "the current branch isn't offered");
  assert.match((seen.picker as string[])[0]!, /^main2 hours agobranchesStephen • Member discount$/);
  assert.match((seen.picker as string[])[2]!, /^origin\/main.*remote branches/);

  const merging = seen.merging as Record<string, unknown>;
  assert.deepEqual(merging.merged, {
    method: "sourceAction",
    target: { type: "conversation", conversationId: "source-panel-fixture" },
    action: { type: "merge", branch: "main" },
  });
  assert.deepEqual(merging.groups, ["Merge Changes1", "Staged Changes1", "Changes0"]);
  assert.equal(merging.message, "Merge branch 'main' into brigadier/912a9469/session");
  assert.equal(merging.commit, "Continue");
  assert.equal(merging.commitDisabled, true, "Continue waits for the conflicts");
  assert.equal(merging.branch, "brigadier/912a9469/session (merging)2");
  assert.deepEqual(merging.toast, [["There are merge conflicts. Resolve them before committing your changes.", ["Show Changes"]]]);
  assert.deepEqual(merging.rowActions, ["Open File", "Stage Changes"]);
  assert.deepEqual(seen.commitSub, ["Commit", "Commit (Amend)", "Abort Merge"]);
  assert.equal(seen.openedTab, "conflict:src/cart/total.ts");
  assert.equal(seen.asked, "Stage total.ts with its conflicts?");
  assert.deepEqual(seen.resolved, { groups: ["Staged Changes2", "Changes0"], commit: "Continue" });
  const committed = seen.committed as { message: string | null; amend: boolean; push: boolean };
  assert.deepEqual([committed.message, committed.amend, committed.push], ["Merge branch 'main' into brigadier/912a9469/session", false, false]);
  assert.equal(seen.afterAbort, "", "an aborted merge's message leaves the box");
  assert.equal(seen.keptMessage, "Unsent message", "closing Source keeps the unsent commit message");
  // Nothing to commit, the change already committed on the branch: the words say so, and only the branch shows (#490).
  assert.deepEqual(seen.clean, {
    box: false,
    commit: false,
    branch: "brigadier/912a9469/session1",
    empty: ["Nothing to commit", "The changes are committed on this branch (1 commit not pushed)."],
  });
  assert.deepEqual(seen.changedAgain, { box: true, commit: "Commit" });
});
