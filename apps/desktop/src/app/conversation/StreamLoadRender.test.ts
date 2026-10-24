import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Phase = { renders: Record<string, number>; deltas?: number };

// The whole app on a long session whose turn streams a word at a time (fixtures/stream-load.tsx):
// each streamed word draws the turn it adds to, not every turn in view, and not the composer or
// the approval waiting in its place, which draw only as the "Working for" clock ticks.
test("a streamed word draws its own turn, not the thread, the composer or the approval card", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "stream-load.html?drive=1&turns=40&rate=20&phase=2000", "stream-load-result", 110000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined);
  const stream = seen.stream as Phase;
  const approval = seen.streamApproval as Phase;
  assert.equal(seen.approvalShown, true);
  const words = stream.deltas ?? 0;
  assert.ok(words >= 20, `it streamed (${words} words)`);
  // Thirty turns are in view: a word drew each of them again before.
  for (const phase of [stream, approval]) {
    assert.ok((phase.renders.RequestBlock ?? 0) <= words * 1.5, `turns drawn: ${phase.renders.RequestBlock} for ${words} words`);
    assert.ok((phase.renders.ConversationComposer ?? 0) <= 6, `composer drawn ${phase.renders.ConversationComposer} times in 2 s`);
  }
  assert.ok((approval.renders.ApprovalAction ?? 0) <= 6, `approval card drawn ${approval.renders.ApprovalAction} times in 2 s`);
});
