import assert from "node:assert/strict";
import { test } from "node:test";

import type { OverrideRule } from "@/ipc/generated";
import { isNoWorkRule, noWorkRule } from "@/lib/workRules";
import { isNewModel, isNewModelRule, isWorkerRule, mayWork } from "@/state/providers";

const OPUS = { provider: "claude" as const, id: "opus" };
const target = { type: "model" as const, ...OPUS };

/** The rule the Routing page's old "Models Brigadier can give work to" switch wrote. */
const saved: OverrideRule = {
  id: "0b7c",
  effect: "never",
  target,
  categories: ["scout", "research", "implement", "review", "merge", "verify", "operate", "design"],
  areas: [],
  projectId: null,
  createdAtMs: 1,
};

test("a model kept from work by the old switch has a Don't give work rule, and still gets no work", () => {
  assert.equal(isNoWorkRule(saved), true);
  assert.equal(mayWork([saved], OPUS), false);
  assert.equal(isNewModel([saved], OPUS), false);
});

test("Rules' Don't give work writes the same rule the old switch did", () => {
  const added = noWorkRule(target, "r", 2);
  assert.equal(isWorkerRule(added, OPUS), true);
  assert.deepEqual({ ...added, id: saved.id, createdAtMs: 1 }, saved);
});

test("a model seen after its agent's first list shows as new, kept from work by its rule", () => {
  const fresh = { ...saved, id: "new-model-0192" };
  assert.equal(isNewModelRule(fresh), true);
  assert.equal(isNewModel([fresh], OPUS), true);
  assert.equal(mayWork([fresh], OPUS), false);
  // Removing its rule allows it.
  assert.equal(mayWork([], OPUS), true);
});

test("a never rule for some kinds of work only stays an ordinary rule", () => {
  const reviews: OverrideRule = { ...saved, categories: ["review"] };
  assert.equal(isNoWorkRule(reviews), false);
  assert.equal(isNoWorkRule({ ...saved, projectId: "p" }), false);
});
