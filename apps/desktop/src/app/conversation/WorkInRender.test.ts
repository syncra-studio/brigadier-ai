import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("Work in offers New worktree (the default) and Local checkout, and each asks for its own setup", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "work-in.html", "work-in-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen.worktreePills, ["New worktree", "feature-x"]);
  assert.deepEqual(seen.rows, [["New worktree", "true"], ["Local checkout", "false"]]);
  const worktree = seen.worktreeSetup as { repo: string; environment: unknown };
  assert.equal(worktree.repo, "/work/todo-app");
  assert.deepEqual(worktree.environment, { type: "newWorktree", base: "feature-x", branch: null });
  assert.match(String(seen.worktreeHint), /worktree of its own/);
  // Local checkout: the project's own folder, on the branch it has checked out.
  assert.deepEqual(seen.localPills, ["Local checkout", "feature-x"]);
  assert.deepEqual((seen.localSetup as { environment: unknown }).environment, { type: "localCheckout", branch: "feature-x", createFrom: null });
  assert.match(String(seen.localHint), /works in your checkout/);
  assert.deepEqual(seen.localRows, [["New worktree", "false"], ["Local checkout", "true"]]);
  assert.deepEqual((seen.backSetup as { environment: unknown }).environment, { type: "newWorktree", base: "feature-x", branch: null });
});
