import assert from "node:assert/strict";
import { test } from "node:test";

import { revertRequest } from "@/app/conversation/activity/revert";

const merge = { type: "merged" as const, branch: "brigadier/s", base: "main", commits: 1, askedIn: null, mergeCommit: "f00dfeed1234" };

test("Revert asks for a pull request's revert pushed, and a merge made on this Mac taken back here", () => {
  assert.equal(revertRequest({ ...merge, pr: 7 }), "Revert merge f00dfee (PR #7) on main and push it the way it went in");
  assert.equal(revertRequest(merge), "Revert merge f00dfee on main the way it went in");
  // A merge with no commit recorded (before phase 3) has no Revert.
  const { mergeCommit: _none, ...unrecorded } = merge;
  assert.equal(revertRequest(unrecorded), null);
});
