import assert from "node:assert/strict";
import { test } from "node:test";

import { type Job, jobLabel } from "@/app/conversation/job";

const job = (states: Job["steps"][number]["state"][], extra: Partial<Job> = {}): Job => ({
  steps: states.map((state, index) => ({ id: `s${index}`, name: ["Building", "Reviewing", "Checks", "Fixing"][index]!, state })),
  ...extra,
});

test("the job's line names the step at work, how far it is, and a failure first", () => {
  assert.equal(jobLabel(job(["done", "active", "pending"])), "Reviewing");
  const counted = job(["done", "active"]);
  assert.equal(jobLabel({ steps: [counted.steps[0]!, { ...counted.steps[1]!, count: "3 of 5" }] }), "Reviewing · 3 of 5");
  assert.equal(jobLabel(job(["done", "done", "failed", "active"])), "Checks failed · fixing");
  assert.equal(jobLabel(job(["done", "failed"])), "Reviewing failed");
  const failed = job(["failed"]);
  assert.equal(jobLabel({ steps: [{ ...failed.steps[0]!, failedLabel: "Build failed" }] }), "Build failed");
  // Between steps, the next one.
  assert.equal(jobLabel(job(["done", "pending"])), "Reviewing");
  assert.equal(
    jobLabel(job(["done", "active"], { phase: { number: 2, of: 4, title: "Implement" } })),
    "Phase 2 of 4 · Reviewing",
  );
});
