import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("Work in offers New worktree (the default), Local checkout and Existing branch, and each asks for its own setup", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "work-in.html", "work-in-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen.worktreePills, ["New worktree", "feature-x"]);
  assert.deepEqual(seen.rows, [["New worktree", "true"], ["Local checkout", "false"], ["Existing branch", "false"]]);
  const worktree = seen.worktreeSetup as { repo: string; environment: unknown };
  assert.equal(worktree.repo, "/work/todo-app");
  assert.deepEqual(worktree.environment, { type: "newWorktree", base: "feature-x", branch: null });
  assert.equal(seen.worktreeHint, "", "no worktree explanation below the composer");
  // Local checkout: the project's own folder, on the branch it has checked out.
  assert.deepEqual(seen.localPills, ["Local checkout", "feature-x"]);
  assert.deepEqual((seen.localSetup as { environment: unknown }).environment, { type: "localCheckout", branch: "feature-x", createFrom: null });
  assert.equal(seen.localHint, "", "no checkout explanation below the composer");
  assert.deepEqual(seen.localRows, [["New worktree", "false"], ["Local checkout", "true"], ["Existing branch", "false"]]);
  assert.deepEqual((seen.backSetup as { environment: unknown }).environment, { type: "newWorktree", base: "feature-x", branch: null });
  // Existing branch (goal 9): each branch with the folder it is checked out in; a running
  // worker's isn't offered; the pick is worked on as it is.
  assert.deepEqual(seen.existingPills, ["Existing branch", "Pick a branch"]);
  assert.equal(seen.existingProblem, "Pick the branch to work on.");
  assert.deepEqual(seen.branchRows, ["feature-x/work/todo-app", "fix-login~/.brigadier/s/ab12cd34/work", "main"]);
  assert.deepEqual((seen.existingSetup as { environment: unknown }).environment, { type: "existing", branch: "fix-login" });
});
