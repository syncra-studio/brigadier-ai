import assert from "node:assert/strict";
import { test } from "node:test";

import { mergeProblems, mergeWarning } from "@/app/conversation/activity/mergeChecks";
import type { MergeRecord } from "@/ipc/generated";

const record = (over: Partial<MergeRecord> = {}): MergeRecord => ({
  head: "a1b2c3d4",
  checks: [{ command: "pnpm test", status: "exit 0", onHead: true }],
  reviews: [{ reviewer: "codex", model: "m", from: "0000aaaa", to: "9999bbbb", state: { type: "findings", count: 1 }, fixed: 1, dismissed: 0, open: 0, onHead: false }],
  ci: { sha: "a1b2c3d4", result: "passed", onHead: true },
  proofs: 1,
  verifier: null,
  ...over,
});

test("a merge's warning line names only what is really wrong", () => {
  // A review short of the merged commit, or none at all, is not wrong: the Reviews tab says it.
  assert.equal(mergeWarning(record()), null);
  assert.equal(mergeWarning(record({ reviews: [], checks: [] })), null);
  assert.equal(mergeWarning(record({ checks: [{ command: "pnpm test", status: "exit 1", onHead: true }] })), "Pnpm test failed.");
  const open = record().reviews.map((review) => ({ ...review, open: 2 }));
  assert.equal(mergeWarning(record({ reviews: open, ci: { sha: "a1", result: "failed", onHead: true } })), "2 review findings are still open and CI failed.");
  assert.equal(mergeWarning(record({ verifier: { commit: "a1", result: "failed", onHead: true } })), "The verifier found a problem.");
});

const VENDORS = { claude: "Claude", codex: "Codex" };

test("the Merging step lists only what is really wrong with the merged commit", () => {
  // Checks on an earlier version, a review short of the merged commit, proofs: none of it lists.
  assert.deepEqual(mergeProblems(record({ checks: [{ command: "pnpm test", status: "exit 0", onHead: false }] }), VENDORS), []);
  const open = record().reviews.map((review) => ({ ...review, open: 1 }));
  assert.deepEqual(
    mergeProblems(record({ checks: [{ command: "pnpm test", status: "exit 1", onHead: true }], reviews: open, ci: { sha: "a1b2c3d4", result: "failed", onHead: true } }), VENDORS),
    ["pnpm test: exit 1", "Codex review of 0000aaa..9999bbb: one finding still open", "CI failed on a1b2c3d"],
  );
});

test("checks that failed on earlier code don't read as the merged commit failing", () => {
  // #497: a run that failed and later passed under a longer command, the full checks before
  // the last commit, a run at the base commit on purpose; CI passed on the merged commit.
  const checks = [
    { command: "pnpm typecheck && pnpm test a.test.ts", status: "exit 1", onHead: false },
    { command: "pnpm typecheck && pnpm test a.test.ts b.test.ts", status: "exit 0", onHead: false },
    { command: "cargo test -p brigadier-core --lib -- two tests", status: "exit 101", onHead: false },
    { command: "tools/full-checks.sh", status: "exit 101", onHead: false },
    { command: "cd /tmp/base-src && cargo test -p brigadier-core --lib -- two tests", status: "exit 101", onHead: false },
  ];
  assert.equal(mergeWarning(record({ checks })), null);
  assert.deepEqual(mergeProblems(record({ checks }), VENDORS), []);
  // The same failure on the merged code still reads.
  const onHead = [...checks, { command: "tools/full-checks.sh", status: "exit 101", onHead: true }];
  assert.equal(mergeWarning(record({ checks: onHead })), "Tools/full-checks.sh failed.");
  assert.deepEqual(mergeProblems(record({ checks: onHead }), VENDORS), ["tools/full-checks.sh: exit 101"]);
});
