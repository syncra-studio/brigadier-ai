import assert from "node:assert/strict";
import { test } from "node:test";

import { redoRequest, revertRequest } from "@/app/conversation/activity/revert";

const merge = { type: "merged" as const, branch: "brigadier/s", base: "main", commits: 1, askedIn: null, mergeCommit: "f00dfeed1234" };

test("Revert asks for a pull request's revert pushed, and a merge made on this Mac taken back here", () => {
  assert.equal(revertRequest({ ...merge, pr: 7 }), "Revert merge f00dfee (PR #7) on main and push it the way it went in");
  assert.equal(revertRequest(merge), "Revert merge f00dfee on main the way it went in");
  // A merge with no commit recorded (before phase 3) has no Revert.
  const { mergeCommit: _none, ...unrecorded } = merge;
  assert.equal(revertRequest(unrecorded), null);
});

test("Revert asks for the revert of a merge made on this Mac and pushed since pushed too", () => {
  // #496: merged into main on this Mac, then main pushed to origin/main.
  const local = { ...merge, upstream: { name: "origin/main", behind: 0 } };
  assert.equal(revertRequest(local), "Revert merge f00dfee on main the way it went in");
  assert.equal(revertRequest(local, true), "Revert merge f00dfee on main and push it the way it went in");
  assert.equal(revertRequest({ ...local, pr: 7 }, true), "Revert merge f00dfee (PR #7) on main and push it the way it went in");
});

test("Redo asks for a reverted merge's revert reverted the way it went in, and only while it stays reverted", () => {
  const undo = { revertedBy: "beefcafe5678", revertedAtMs: 1 };
  assert.equal(redoRequest(merge, undo), "Redo merge f00dfee on main: revert its revert beefcaf the way it went in");
  assert.equal(
    redoRequest({ ...merge, pr: 7 }, undo),
    "Redo merge f00dfee (PR #7) on main: revert its revert beefcaf and push it the way it went in",
  );
  assert.equal(redoRequest(merge, null), null, "never reverted");
  assert.equal(redoRequest(merge, { ...undo, redoneBy: "abc", redoneAtMs: 2 }), null, "redone already");
});
