import assert from "node:assert/strict";
import { test } from "node:test";

import { environmentRequest } from "@/app/conversation/draftEnvironment";

const place = { base: null, sessionBranch: "", current: "feature-x", loading: false };

test("a new worktree starts from the picked base, else the checked-out branch, under the name given", () => {
  assert.deepEqual(environmentRequest({ ...place, environment: "newWorktree" }), {
    request: { type: "newWorktree", base: "feature-x", branch: null },
  });
  assert.deepEqual(environmentRequest({ ...place, environment: "newWorktree", base: "main", sessionBranch: " fix-login " }), {
    request: { type: "newWorktree", base: "main", branch: "fix-login" },
  });
  assert.deepEqual(environmentRequest({ ...place, environment: "newWorktree", current: null, loading: true }), { problem: "Reading the repository…" });
  assert.deepEqual(environmentRequest({ ...place, environment: "newWorktree", current: null }), { problem: "Pick the base branch." });
});

test("the local checkout works on the branch the project's folder has checked out, whatever base was picked", () => {
  assert.deepEqual(environmentRequest({ ...place, environment: "localCheckout", base: "main", sessionBranch: "ignored" }), {
    request: { type: "localCheckout", branch: "feature-x", createFrom: null },
  });
  assert.deepEqual(environmentRequest({ ...place, environment: "localCheckout", current: null, loading: true }), { problem: "Reading the repository…" });
  assert.match(
    (environmentRequest({ ...place, environment: "localCheckout", current: null }) as { problem: string }).problem,
    /isn't on a branch/,
  );
});
