import assert from "node:assert/strict";
import { test } from "node:test";

import { checkoutHolder, environmentRequest } from "@/app/conversation/draftEnvironment";
import type { Conversation, Environment, Lifecycle } from "@/ipc/generated";

const place = { base: null, sessionBranch: "", current: "feature-x", loading: false };

const session = (title: string, environment: Environment, lifecycle: Lifecycle = "active", repo = "/repo") =>
  ({ title, kind: "session", lifecycle, setup: { type: "session", repo, environment } }) as unknown as Conversation;

test("a new worktree starts from the picked base, else the checked-out branch, under the name given", () => {
  assert.deepEqual(environmentRequest({ ...place, environment: "newWorktree" }), {
    request: { type: "newWorktree", base: "feature-x", branch: null },
  });
  assert.deepEqual(environmentRequest({ ...place, environment: "newWorktree", base: "main", sessionBranch: " fix-login " }), {
    request: { type: "newWorktree", base: "main", branch: "fix-login" },
  });
  assert.deepEqual(environmentRequest({ ...place, environment: "newWorktree", current: null, loading: true }), { problem: "Reading the repository…", waiting: true });
  assert.deepEqual(environmentRequest({ ...place, environment: "newWorktree", current: null }), { problem: "Pick the base branch." });
});

test("the local checkout works on the branch the project's folder has checked out, whatever base was picked", () => {
  assert.deepEqual(environmentRequest({ ...place, environment: "localCheckout", base: "main", sessionBranch: "ignored" }), {
    request: { type: "localCheckout", branch: "feature-x", createFrom: null },
  });
  assert.deepEqual(environmentRequest({ ...place, environment: "localCheckout", current: null, loading: true }), { problem: "Reading the repository…", waiting: true });
  assert.match(
    (environmentRequest({ ...place, environment: "localCheckout", current: null }) as { problem: string }).problem,
    /isn't on a branch/,
  );
});

test("the local checkout can't start while another session works there; an archived one or a worktree doesn't hold it", () => {
  assert.deepEqual(environmentRequest({ ...place, environment: "localCheckout", holder: "Fix login" }), {
    problem: "“Fix login” already works in this checkout. Archive it, or start this session in a new worktree.",
  });
  assert.deepEqual(environmentRequest({ ...place, environment: "newWorktree", holder: "Fix login" }), {
    request: { type: "newWorktree", base: "feature-x", branch: null },
  });
  const local: Environment = { type: "localCheckout", branch: "main" };
  const worktree: Environment = { type: "newWorktree", base: "main", branch: "s", path: null, start: null };
  assert.equal(checkoutHolder([session("Gone", local, "archived"), session("Away", worktree), session("Other repo", local, "active", "/other")], "/repo"), null);
  assert.equal(checkoutHolder([session("Gone", local, "archived"), session("Resting", local, "hibernated")], "/repo")?.title, "Resting");
});
