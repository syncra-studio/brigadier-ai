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
