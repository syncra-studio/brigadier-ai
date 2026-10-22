import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

/** The fixture's JSON as the page's DOM escaped it. */
const unescape = (dump: string) => dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&");

test("a rebase stopped on conflicts is resolved in the conflict tab, continued commit by commit, and aborted", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(unescape(await renderFixturePage(t, "source-rebase.html", "source-rebase-result", 8000))) as Record<string, unknown>;
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  const stopped = seen.stopped as Record<string, unknown>;
  assert.deepEqual(stopped.groups, ["Merge Changes1", "Staged Changes1", "Changes0"]);
  assert.equal(stopped.branch, "brigadier/912a9469/session (rebasing)");
  assert.equal(stopped.message, "Apply coupons before tax\n\nCoupons now come off the subtotal.");
  assert.equal(stopped.boxDisabled, true, "the replayed commit keeps its own message");
  assert.equal(stopped.commit, "Continue");
  assert.equal(stopped.commitDisabled, true, "Continue waits for the conflicts");
  assert.equal(stopped.step, "Rebasing 2 of 5: Apply coupons before tax");
  assert.deepEqual(stopped.lenses, ["Accept Current Change|Accept Incoming Change|Accept Both Changes|Compare Changes"]);
  assert.deepEqual(seen.commitSub, ["Commit", "Commit (Amend)", "Abort Rebase"]);
  assert.deepEqual(seen.branchDisabled, [true, true], "no merge or second rebase meanwhile");

  assert.deepEqual(seen.resolved, { groups: ["Staged Changes2", "Changes0"], commit: "Continue" });
  const continued = seen.continued as Record<string, unknown>;
  assert.deepEqual(continued.call, {
    method: "sourceAction",
    target: { type: "conversation", conversationId: "source-rebase-fixture" },
    action: { type: "continueRebase" },
  });
  assert.equal(continued.step, "Rebasing 3 of 5: Show the discount on the receipt");
  assert.equal(continued.message, "Show the discount on the receipt");
  assert.deepEqual(continued.toasts, ["There are merge conflicts. Resolve them before committing your changes."]);
  assert.deepEqual(seen.aborted, { branch: "brigadier/912a9469/session", box: false });
});
