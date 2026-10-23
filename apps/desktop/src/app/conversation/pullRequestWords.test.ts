import assert from "node:assert/strict";
import { test } from "node:test";

import { PULL_REQUEST_TONES, pullRequestLook } from "@/app/conversation/pullRequestWords";
import type { PullRequest } from "@/ipc/generated";

const base: PullRequest = { number: 7, title: "Add notes", url: "https://github.com/o/n/pull/7", state: "open", ci: "passing", approved: true, mergeable: true };

const cases: [string, Partial<PullRequest>, string, string][] = [
  ["merged beats all", { state: "merged", ci: "failing", conflicts: true }, "Merged", "text-pr-purple"],
  ["closed beats checks", { state: "closed", ci: "failing" }, "Closed", "text-muted-foreground"],
  ["draft beats failed checks", { state: "draft", ci: "failing" }, "Draft", "text-muted-foreground"],
  ["failed beats review and conflicts", { ci: "failing", changesRequested: true, conflicts: true }, "Checks failed", "text-destructive"],
  ["running beats review and conflicts", { ci: "pending", changesRequested: true, conflicts: true }, "Checks running", "text-pr-yellow"],
  ["approved, green and mergeable is ready", {}, "Ready", "text-success"],
  ["no approval needs review", { approved: false }, "Review needed", "text-pr-yellow"],
  ["unknown mergeability needs review", { mergeable: false }, "Review needed", "text-pr-yellow"],
  ["conflicts need review", { conflicts: true, mergeable: false }, "Review needed", "text-pr-yellow"],
  ["changes requested needs review", { changesRequested: true, approved: false }, "Review needed", "text-pr-yellow"],
];
for (const [name, patch, word, colour] of cases) {
  test(name, () => {
    const look = pullRequestLook({ ...base, ...patch });
    assert.equal(look.word, word);
    assert.equal(PULL_REQUEST_TONES[look.tone], colour);
  });
}

test("no checks is not green", () => {
  const pr = { ...base };
  delete pr.ci;
  assert.equal(pullRequestLook(pr).word, "Review needed");
});
