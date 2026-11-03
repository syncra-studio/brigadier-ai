import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("Work in offers New worktree (the default), Local checkout and Existing worktree, and each asks for its own setup", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "work-in.html", "work-in-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen.worktreePills, ["New worktree", "feature-x"]);
  assert.deepEqual(seen.rows, [["New worktree", "true"], ["Local checkout", "false"], ["Existing worktree", "false"]]);
  const worktree = seen.worktreeSetup as { repo: string; environment: unknown };
  assert.equal(worktree.repo, "/work/todo-app");
  assert.deepEqual(worktree.environment, { type: "newWorktree", base: "feature-x", branch: null });
  assert.equal(seen.worktreeHint, "", "no worktree explanation below the composer");
  // Local checkout: the project's own folder, on the branch it has checked out.
  assert.deepEqual(seen.localPills, ["Local checkout", "feature-x"]);
  assert.deepEqual((seen.localSetup as { environment: unknown }).environment, { type: "localCheckout", branch: "feature-x", createFrom: null });
  assert.equal(seen.localHint, "", "no checkout explanation below the composer");
  assert.deepEqual(seen.localRows, [["New worktree", "false"], ["Local checkout", "true"], ["Existing worktree", "false"]]);
  assert.deepEqual((seen.backSetup as { environment: unknown }).environment, { type: "newWorktree", base: "feature-x", branch: null });
  // Existing worktree (goal 9, D130): the other worktrees, each by the session working there or
  // its folder's name, its branch below (D184); the project's own checkout and a running
  // worker's aren't offered; the pick is the folder, its pill the same name.
  assert.deepEqual(seen.existingPills, ["Existing worktree", "Pick a worktree"]);
  assert.equal(seen.existingProblem, "Pick the worktree to work in.");
  assert.deepEqual(seen.worktreeRows, ["Fix the loginfix-login", "todo-app-hotfixhotfix"]);
  assert.equal(seen.pickedPill, "Fix the login");
  assert.deepEqual((seen.existingSetup as { environment: unknown }).environment, { type: "existing", path: "/Users/me/.brigadier/s/ab12cd34/work" });
});
