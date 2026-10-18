import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The proof the thread attached shows in the merge message, under the answer: the screenshot
// opens in the viewer, the recording plays in place, each with its caption; no row of its own.
test("the merge message shows the thread's screenshot and recording", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "merge-message.html?drive=1", "merge-message-result", 15000)) as Record<string, unknown>;
  assert.deepEqual(seen.proofs, [
    { kind: "image", image: "The install section, fixed", video: null, caption: "The install section, fixed" },
    { kind: "video", image: null, video: "data:video/webm;", caption: null },
  ]);
  assert.equal(seen.underAnswer, true);
  assert.equal(seen.rows, 0);
  assert.deepEqual(seen.read, ["proof-shot", "proof-video"]);
});

// Revert on the merge message sends the thread its fixed request once, shown as the user's.
test("Revert on the merge message sends the fixed request once", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "merge-message.html?drive=1", "merge-message-result", 15000)) as { revert: Record<string, unknown> };
  assert.deepEqual(seen.revert, {
    shown: "Revert",
    sent: ["Revert merge f00dfee (PR #7) on main and push it the way it went in"],
    after: "Revert asked",
  });
});

// The merge message lists what was checked of the merged commit, a review short of it marked;
// the project's first merge on its own says so once, with Revert beside the line.
test("the merge message shows what was checked and the first merge on its own", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "merge-message.html?drive=1", "merge-message-result", 15000)) as Record<string, unknown>;
  assert.deepEqual(seen.checked, [
    "pnpm test: passed on a1b2c3d",
    "! Codex review of 0000aaa..9999bbb: 2 findings, 1 fixed, 1 dismissed, 0 open, not up to the merged commit",
    "CI passed on a1b2c3d",
    "2 screenshots or recordings",
  ]);
  assert.equal(seen.firstAuto, "Merged on its own, as your setting has it.");
  assert.equal(seen.revertBeside, true);
  const later = JSON.parse(await renderFixturePage(t, "merge-message.html?drive=1&first=0", "merge-message-result", 15000)) as Record<string, unknown>;
  assert.equal(later.firstAuto, null, "quiet after the first");
});
